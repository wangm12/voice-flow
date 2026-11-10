#!/usr/bin/env python3
"""Fetch only text files at source revisions proven by locked package metadata."""
from __future__ import annotations
import argparse, hashlib, importlib.util, json, os, re, urllib.error, urllib.request
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote, urlsplit

_CLASSIFIER_SPEC = importlib.util.spec_from_file_location('voiceflow_notice_classifier', Path(__file__).with_name('generate_notices.py'))
if not _CLASSIFIER_SPEC or not _CLASSIFIER_SPEC.loader:
    raise RuntimeError('could not load the shared notice classifier')
_CLASSIFIER = importlib.util.module_from_spec(_CLASSIFIER_SPEC)
_CLASSIFIER_SPEC.loader.exec_module(_CLASSIFIER)

ROOT = Path(__file__).resolve().parents[1]
PARENT = ROOT.parent
CARGO_META = PARENT / 'distribution-dependency-license-inventory-v1/cargo-metadata.json'
V1_INDEX = PARENT / 'distribution-notices-impl-v1/project-overlay/ThirdPartyLicenses/DependencyTexts/license-index.json'
OVERLAY = ROOT / 'project-overlay'
INDEX_PATH = OVERLAY / 'ThirdPartyLicenses/DependencyTexts/license-index.json'
TEXT_ROOT = OVERLAY / 'ThirdPartyLicenses/DependencyTexts/by-sha256'
EVIDENCE = ROOT / 'evidence/upstream-fetch'
CAP = 4 * 1024 * 1024
MAX_BODY = 160 * 1024
RUST_FILES = {
    'alloc-stdlib': ['LICENSE'],
    'block2': ['LICENSE.md'],
    'dasp_sample': ['LICENSE-APACHE', 'LICENSE-MIT'],
    'defmt-parser': ['LICENSE-APACHE', 'LICENSE-MIT'],
    'dispatch2': ['LICENSE.md'],
    'objc2': ['LICENSE.md'],
    'objc2-app-kit': ['LICENSE.md'],
    'objc2-audio-toolbox': ['LICENSE.md'],
    'objc2-av-foundation': ['LICENSE.md'],
    'objc2-core-audio': ['LICENSE.md'],
    'objc2-core-audio-types': ['LICENSE.md'],
    'objc2-core-foundation': ['LICENSE.md'],
    'objc2-core-graphics': ['LICENSE.md'],
    'objc2-encode': ['LICENSE.txt', 'LICENSE.md'],
    'objc2-exception-helper': ['LICENSE.txt', 'LICENSE.md'],
    'objc2-foundation': ['LICENSE.md'],
    'objc2-io-surface': ['LICENSE.md'],
    'objc2-web-kit': ['LICENSE.md'],
    'realfft': ['LICENSE-MIT', 'LICENSE', 'LICENSE.txt'],
    'selectors': ['LICENSE', 'LICENSE.txt', 'LICENSE.md', 'selectors/LICENSE', 'selectors/LICENSE.txt', 'selectors/LICENSE.md'],
    'siphasher': ['COPYING'],
    'unic-char-property': ['LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT.md'],
    'unic-char-range': ['LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT.md'],
    'unic-common': ['LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT.md'],
    'unic-ucd-ident': ['LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT.md'],
    'unic-ucd-version': ['LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT.md'],
}
NPM_FILES = {
    '@tauri-apps/plugin-dialog': ['LICENSE_APACHE-2.0', 'LICENSE_MIT'],
    'react-remove-scroll-bar': ['LICENSE'],
}
OBJC2_MIT_ONLY = {'objc2', 'block2', 'objc2-foundation', 'objc2-encode'}
OBJC2_TRIO_LICENSED = {
    'dispatch2', 'objc2-app-kit', 'objc2-audio-toolbox', 'objc2-av-foundation', 'objc2-core-audio',
    'objc2-core-audio-types', 'objc2-core-foundation', 'objc2-core-graphics',
    'objc2-exception-helper', 'objc2-io-surface', 'objc2-web-kit',
}

def digest(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()

def save(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.write_bytes(data)
    path.chmod(0o600)
    path.parent.chmod(0o700)

def github_slug(value: str | None) -> str | None:
    if not value:
        return None
    value = value.strip().removeprefix('git+').removesuffix('.git')
    if value.startswith('git@github.com:'):
        value = 'https://github.com/' + value.split(':', 1)[1]
    parsed = urlsplit(value)
    if parsed.scheme not in {'https', 'http'} or (parsed.hostname or '').casefold() != 'github.com':
        return None
    path = parsed.path.strip('/').removesuffix('.git')
    if len(path.split('/')) != 2 or any(not re.fullmatch(r'[A-Za-z0-9_.-]+', p) for p in path.split('/')):
        return None
    return path

def http_get(url: str, budget: dict, *, purpose: str) -> tuple[bytes, dict]:
    request = urllib.request.Request(url, headers={'User-Agent':'VoiceFlow-Private-Notice-Inventory/2.0', 'Accept':'application/json, text/plain, text/markdown, */*'})
    at = datetime.now(timezone.utc).isoformat()
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            body = response.read(MAX_BODY + 1)
            status = response.status
            final_url = response.geturl()
            content_type = response.headers.get('Content-Type')
    except urllib.error.HTTPError as e:
        body = e.read(MAX_BODY + 1)
        status, final_url, content_type = e.code, e.geturl(), e.headers.get('Content-Type')
    if len(body) > MAX_BODY:
        raise RuntimeError(f'response too large for text-only request: {url}')
    if budget['bytes'] + len(body) > CAP:
        raise RuntimeError('4 MiB total network body cap reached')
    budget['bytes'] += len(body)
    rec = {'url':url, 'final_url':final_url, 'status':status, 'content_type':content_type, 'bytes':len(body), 'sha256':digest(body), 'requested_at_utc':at, 'purpose':purpose}
    return body, rec

def text_kind(filename: str, body: bytes) -> str:
    try:
        text = body.decode('utf-8')
    except UnicodeDecodeError:
        raise ValueError('license candidate is not UTF-8 text')
    if '\x00' in text or text.lstrip().lower().startswith('<!doctype html') or text.lstrip().lower().startswith('<html'):
        raise ValueError('license candidate does not look like plain text')
    path = Path(filename)
    if not _CLASSIFIER.is_license_path(path) and path.name.casefold() not in {'notice', 'notice.txt', 'copyright'}:
        # Canonical source names such as MIT.txt are license texts even though
        # they do not use a package's conventional LICENSE-* basename.
        path = Path('LICENSE-' + path.name)
    kind = _CLASSIFIER.classify_text(path, body)
    if kind == 'other_metadata':
        raise ValueError('candidate is not a recognized license/notice text path')
    return kind


def pinned_raw_url(repository: str, revision: str, source_path: str) -> str:
    """Build a raw URL only from an exact commit hash, never a package version/tag."""
    if not re.fullmatch(r'[0-9a-fA-F]{40}', revision):
        raise ValueError('source revision must be a full 40-character commit id')
    if not source_path or source_path.startswith('/') or '..' in Path(source_path).parts:
        raise ValueError('unsafe upstream text path')
    slug = github_slug(repository)
    if not slug:
        raise ValueError('repository must be an exact public GitHub origin')
    return f'https://raw.githubusercontent.com/{slug}/{revision}/{source_path}'

def validate_npm_registry_identity(name: str, version: str, lock_integrity: str, metadata: dict) -> tuple[str, str]:
    """Return verified repository slug and gitHead after exact registry-lock checks."""
    if metadata.get('name') != name or metadata.get('version') != version:
        raise ValueError('npm registry name/version mismatch')
    dist = metadata.get('dist') or {}
    if dist.get('integrity') != lock_integrity:
        raise ValueError('npm registry integrity does not match package-lock')
    repo_obj = metadata.get('repository')
    repo_url = repo_obj.get('url') if isinstance(repo_obj, dict) else repo_obj
    slug = github_slug(repo_url)
    revision = metadata.get('gitHead')
    if not slug or not isinstance(revision, str) or not re.fullmatch(r'[0-9a-fA-F]{40}', revision):
        raise ValueError('npm registry metadata lacks a verifiable repository/gitHead')
    return slug, revision


def attach_objc2_canonical_mit_texts(index: dict, bindings: dict[str, dict], overlay_root: Path) -> int:
    """Bind one verified canonical MIT body to each pinned objc2 declaration that permits it."""
    canonical_file = None
    canonical_source_url = 'https://spdx.org/licenses/MIT.txt'
    canonical_hash = None
    for binding in bindings.values():
        for item in binding.get('files', []):
            if item.get('source_url') == canonical_source_url and item.get('kind') == 'license_text':
                canonical_file = overlay_root / item['output_path']
                canonical_hash = item.get('sha256')
                break
        if canonical_file:
            break
    if not canonical_file or not canonical_file.is_file():
        return 0
    canonical_bytes = canonical_file.read_bytes()
    if digest(canonical_bytes) != canonical_hash or text_kind('MIT.txt', canonical_bytes) != 'license_text':
        raise ValueError('canonical MIT text hash or classification does not match its pinned source record')

    attached = 0
    for row in index.get('packages', []):
        name = row.get('name')
        if row.get('ecosystem') != 'rust' or name not in OBJC2_MIT_ONLY | OBJC2_TRIO_LICENSED:
            continue
        key = f"{name}@{row['version']}"
        binding = bindings.get(key)
        if not binding or binding.get('status') != 'exact_revision_verified':
            continue
        if binding.get('repository_url') != 'https://github.com/madsmtm/objc2':
            continue
        revision = binding.get('source_revision')
        if not isinstance(revision, str) or not re.fullmatch(r'[0-9a-fA-F]{40}', revision):
            continue
        expression = row.get('license_expression') or ''
        alternatives = {part.strip() for part in expression.split(' OR ')}
        if 'MIT' not in alternatives:
            continue
        guide = next((
            item for item in binding.get('files', [])
            if item.get('source_path') == 'LICENSE.md'
            and item.get('source_revision') == revision
            and item.get('source_url') == f'https://raw.githubusercontent.com/madsmtm/objc2/{revision}/LICENSE.md'
        ), None)
        if not guide:
            continue
        guide_path = overlay_root / guide['output_path']
        if not guide_path.is_file():
            continue
        guide_bytes = guide_path.read_bytes()
        if digest(guide_bytes) != guide.get('sha256'):
            raise ValueError(f'pinned objc2 guide hash mismatch for {key}')
        if text_kind('LICENSE.md', guide_bytes) != 'license_reference_metadata':
            continue
        guide_text = guide_bytes.decode('utf-8', errors='strict').casefold()
        if name in OBJC2_MIT_ONLY:
            group = 'the crates `objc2`, `block2`, `objc2-foundation` and `objc2-encode`'
            if group not in guide_text or 'currently' not in guide_text or '[the mit license][mit]' not in guide_text:
                continue
        elif not all(token in guide_text for token in ('all other crates are trio-licensed', '[zlib]', '[apache-2.0]', '[mit]')):
            continue
        if any(item.get('source_url') == canonical_source_url and item.get('sha256') == canonical_hash for item in binding.get('files', [])):
            continue
        binding.setdefault('files', []).append({
            'source_path': 'canonical-license-texts/MIT.txt',
            'output_path': f'ThirdPartyLicenses/DependencyTexts/by-sha256/{canonical_hash}.txt',
            'sha256': canonical_hash,
            'bytes': len(canonical_bytes),
            'kind': 'license_text',
            'source_url': canonical_source_url,
            'source_revision': None,
            'package_source_revision': revision,
            'license_expression_id': 'MIT',
            'guide_source_url': guide['source_url'],
            'guide_sha256': guide['sha256'],
            'source_relationship': 'Canonical MIT terms, not bytes from this package archive. The pinned upstream LICENSE.md names MIT as an applicable option for this exact package source; the separate Apple SDK-derived-work discussion remains outside this text attribution record.',
        })
        attached += 1
    return attached


def merge_pinned_text_bindings(index: dict, bindings: dict[str, dict], overlay_root: Path) -> dict:
    """Merge source-bound text candidates without dropping saved metadata or bodies."""
    for row in index.get('packages', []):
        if row.get('ecosystem') == 'rust' and (row.get('first_party') or row.get('workspace_package') or str(row.get('source', '')).startswith('workspace:')):
            row['first_party'] = True
            row['first_party_license_expression_absent'] = not bool(row.get('license_expression'))
        key = f"{row.get('name')}@{row.get('version')}" if row.get('ecosystem') == 'rust' else row.get('name')
        binding = bindings.get(key)
        if not binding:
            if row.get('first_party') and not row.get('license_expression') and not _CLASSIFIER.has_license_text(row):
                row['text_status'] = 'first_party_license_undeclared'
                row['unresolved_reason'] = 'First-party workspace package; project-wide license declaration was not found. This is not a third-party dependency notice gap.'
            continue
        if row.get('ecosystem') == 'rust' and binding.get('source_revision'):
            row['source_revision'] = binding['source_revision']
            row['exact_source_revision_binding'] = {k: v for k, v in binding.items() if k != 'files'}
        elif row.get('ecosystem') == 'npm' and binding.get('registry_git_head'):
            row['source_revision'] = binding['registry_git_head']
            row['exact_source_revision_binding'] = {k: v for k, v in binding.items() if k != 'files'}

        existing = row.setdefault('texts', [])
        for old in existing:
            old_path = overlay_root / old.get('output_path', '')
            if old_path.is_file():
                data = old_path.read_bytes()
                if len(data) != old.get('bytes') or digest(data) != old.get('sha256'):
                    raise ValueError(f"saved text hash/size mismatch for {row.get('name')}@{row.get('version')}")
                classification_path = Path('LICENSE-' + Path(old.get('source_path', '')).name) if str(old.get('source_path', '')).startswith('canonical-license-texts/') else Path(old.get('source_basename') or old.get('source_path') or 'LICENSE')
                old['kind'] = _CLASSIFIER.classify_text(classification_path, data)
        existing_keys = {(t.get('output_path'), t.get('sha256')) for t in existing}
        for item in binding.get('files', []):
            key3 = (item['output_path'], item['sha256'])
            if key3 in existing_keys:
                continue
            output_file = overlay_root / item['output_path']
            body = output_file.read_bytes()
            if len(body) != item['bytes'] or digest(body) != item['sha256']:
                raise ValueError(f"pinned source text hash/size mismatch for {row.get('name')}@{row.get('version')}")
            basename = Path(item['source_path']).name
            classification_path = Path('LICENSE-' + basename) if item['source_path'].startswith('canonical-license-texts/') else Path(basename)
            kind = _CLASSIFIER.classify_text(classification_path, body)
            record = {k: item[k] for k in ('source_path', 'output_path', 'sha256', 'bytes')}
            record['kind'] = kind
            record['source_basename'] = Path(item['source_path']).name
            record['source_url'] = item['source_url']
            record['source_revision'] = item.get('source_revision')
            evidence = _CLASSIFIER.license_classification_evidence(classification_path, body, kind)
            if evidence:
                record['classification_evidence'] = evidence
            for extra in ('package_source_revision', 'source_relationship', 'license_expression_id', 'guide_source_url', 'guide_sha256'):
                if extra in item:
                    record[extra] = item[extra]
            existing.append(record)
            existing_keys.add(key3)
        canonical = next((t for t in binding.get('files', []) if t.get('license_expression_id') == 'MIT'), None) if row.get('name') in OBJC2_MIT_ONLY | OBJC2_TRIO_LICENSED else None
        if canonical:
            row['canonical_license_choice'] = {
                'license_id': canonical['license_expression_id'],
                'text_sha256': canonical['sha256'],
                'canonical_source_url': canonical['source_url'],
                'guide_source_url': canonical.get('guide_source_url'),
                'guide_source_revision': canonical.get('package_source_revision'),
                'guide_sha256': canonical.get('guide_sha256'),
                'source_relationship': canonical['source_relationship'],
            }
        if _CLASSIFIER.has_license_text(row):
            row['text_status'] = 'license_text_found_from_same_repo_release_peer' if row.get('text_source_relationship') else 'license_text_found'
            row['unresolved_reason'] = None
        elif row.get('first_party') and not row.get('license_expression'):
            row['text_status'] = 'first_party_license_undeclared'
            row['unresolved_reason'] = 'First-party workspace package; project-wide license declaration was not found. This is not a third-party dependency notice gap.'
        elif existing:
            row['text_status'] = _CLASSIFIER.text_status(existing)

    packages = index.get('packages', [])
    third_party_missing = [r for r in packages if not _CLASSIFIER.has_license_text(r) and not r.get('first_party')]
    first_party_undeclared = [r for r in packages if r.get('first_party_license_expression_absent')]
    full = sum(_CLASSIFIER.has_license_text(r) for r in packages)
    index.setdefault('summary', {}).update({
        'records_with_full_license_text': full,
        'records_without_full_license_text': len(packages) - full,
        'third_party_records_without_full_license_text': len(third_party_missing),
        'first_party_undeclared_license_records': len(first_party_undeclared),
        'unique_dependency_files': len(list((overlay_root / 'ThirdPartyLicenses/DependencyTexts/by-sha256').glob('*.txt'))),
    })
    return index

def load_local_inputs(index_path: Path = V1_INDEX, cargo_metadata_path: Path = CARGO_META):
    idx = json.loads(index_path.read_text())
    cargo = json.loads(cargo_metadata_path.read_text())
    rust = {(p['name'], p['version']):p for p in cargo['packages']}
    return idx, cargo, rust

def main(argv: list[str] | None = None) -> int:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo',required=True,type=Path,help='read-only VoiceFlow repository root')
    parser.add_argument('--inventory-index',type=Path,default=V1_INDEX,help='frozen dependency license index to extend')
    parser.add_argument('--cargo-metadata',type=Path,default=CARGO_META,help='offline Cargo metadata JSON with exact package source roots')
    parser.add_argument('--overlay-root',type=Path,default=OVERLAY,help='notice overlay output root')
    parser.add_argument('--evidence-root',type=Path,default=EVIDENCE,help='private fetch evidence root')
    args=parser.parse_args(argv)
    repo_root=args.repo.resolve(strict=True)
    inventory_index=args.inventory_index.resolve(strict=True)
    cargo_metadata_path=args.cargo_metadata.resolve(strict=True)
    overlay_root=args.overlay_root.absolute()
    evidence_root=args.evidence_root.absolute()
    idx, cargo, cargo_pkgs = load_local_inputs(inventory_index,cargo_metadata_path)
    index_path=overlay_root/'ThirdPartyLicenses/DependencyTexts/license-index.json'
    text_root=overlay_root/'ThirdPartyLicenses/DependencyTexts/by-sha256'
    if digest((repo_root/'src-tauri/Cargo.lock').read_bytes()) != idx['inputs']['cargo_lock']['sha256']:
        raise RuntimeError('current Cargo.lock differs from frozen inventory input')
    if digest((repo_root/'package-lock.json').read_bytes()) != idx['inputs']['package_lock']['sha256']:
        raise RuntimeError('current package-lock.json differs from frozen inventory input')
    budget = {'bytes':0}
    requests: dict[str,dict] = {}
    package_bindings: dict[str,dict] = {}
    registry_metadata = {}
    lock = json.loads((repo_root/'package-lock.json').read_text())
    installed = {}
    for row in idx['packages']:
        if row['ecosystem'] != 'npm' or row['text_status'] == 'license_text_found':
            continue
        name = row['name']
        version = row['version']
        lock_path = row['lock_identity']['lock_path']
        lockrow = lock.get('packages',{}).get(lock_path)
        if not lockrow or lockrow.get('version') != version or lockrow.get('integrity') != row['lock_identity'].get('integrity'):
            raise RuntimeError(f'npm lock identity mismatch: {name}@{version}')
        pkg_path = repo_root / lock_path / 'package.json'
        package_json = json.loads(pkg_path.read_text())
        installed[name] = {'version':package_json.get('version'), 'gitHead':package_json.get('gitHead')}
        endpoint = 'https://registry.npmjs.org/' + quote(name, safe='') + '/' + quote(version, safe='')
        body, rec = http_get(endpoint, budget, purpose='exact npm registry version metadata; verify package-lock integrity and gitHead')
        rec.update({'package':name,'version':version})
        if rec['status'] != 200:
            requests[endpoint] = rec
            package_bindings[name] = {'status':'unresolved_registry_metadata_unavailable','registry_url':endpoint}
            continue
        meta = json.loads(body)
        repo_obj = meta.get('repository')
        repo_url = repo_obj.get('url') if isinstance(repo_obj,dict) else repo_obj
        requests[endpoint] = rec
        meta_name = name.replace('/','__').replace('@','').replace('.','-')+'-'+version+'.json'
        save(evidence_root/'registry-metadata'/meta_name,body)
        try:
            slug, git_head = validate_npm_registry_identity(name,version,lockrow.get('integrity'),meta)
        except ValueError as e:
            package_bindings[name] = {'status':'unresolved_registry_identity_mismatch','registry_url':endpoint,'registry_metadata_sha256':digest(body),'repository_url':repo_url,'reason':str(e)}
            continue
        if not isinstance(git_head,str) or not re.fullmatch(r'[0-9a-fA-F]{40}',git_head):
            package_bindings[name] = {'status':'unresolved_no_exact_git_head','registry_url':endpoint,'registry_metadata_sha256':digest(body),'registry_git_head':None,'repository_url':repo_url}
        elif slug != github_slug(row.get('upstream_repository')):
            package_bindings[name] = {'status':'unresolved_repository_mismatch','registry_url':endpoint,'registry_metadata_sha256':digest(body),'registry_git_head':git_head,'repository_url':repo_url}
        else:
            package_bindings[name] = {'status':'exact_revision_verified','registry_url':endpoint,'registry_metadata_sha256':digest(body),'registry_git_head':git_head,'repository_url':'https://github.com/'+slug,'registry_integrity':lockrow['integrity']}

    for row in idx['packages']:
        if row['ecosystem'] != 'rust' or row['text_status'] == 'license_text_found' or row['name'] == 'voiceflow':
            continue
        p = cargo_pkgs.get((row['name'],row['version']))
        if not p:
            package_bindings[row['name']+'@'+row['version']] = {'status':'unresolved_package_not_in_metadata'}
            continue
        root = Path(p['manifest_path']).parent
        vcs = root/'.cargo_vcs_info.json'
        if not vcs.is_file():
            package_bindings[row['name']+'@'+row['version']] = {'status':'unresolved_no_exact_cargo_vcs_info','registry_url':f"https://crates.io/crates/{row['name']}/{row['version']}",'lock_checksum':row['lock_identity'].get('checksum'),'repository_url':row.get('upstream_repository')}
            continue
        vcs_bytes = vcs.read_bytes()
        vcs_obj = json.loads(vcs_bytes)
        commit = vcs_obj.get('git',{}).get('sha1')
        if not isinstance(commit,str) or not re.fullmatch(r'[0-9a-fA-F]{40}',commit):
            package_bindings[row['name']+'@'+row['version']] = {'status':'unresolved_invalid_cargo_vcs_revision'}
            continue
        slug = github_slug(p.get('repository'))
        if not slug or slug != github_slug(row.get('upstream_repository')):
            package_bindings[row['name']+'@'+row['version']] = {'status':'unresolved_repository_mismatch','source_revision':commit}
            continue
        filenames = RUST_FILES.get(row['name'])
        if not filenames:
            package_bindings[row['name']+'@'+row['version']] = {'status':'unresolved_no_candidate_file_rule','source_revision':commit}
            continue
        relpath = vcs_obj.get('path_in_vcs') or ''
        if relpath.startswith('/') or '..' in Path(relpath).parts:
            raise RuntimeError(f'unsafe cargo path_in_vcs for {row["name"]}')
        binding_key = row['name']+'@'+row['version']
        package_bindings[binding_key] = {'status':'exact_revision_verified','source_revision':commit,'repository_url':'https://github.com/'+slug,'path_in_vcs':relpath,'cargo_vcs_info_sha256':digest(vcs_bytes),'lock_checksum':row['lock_identity'].get('checksum'),'registry_url':f"https://crates.io/crates/{row['name']}/{row['version']}",'files':[]}
        for filename in filenames:
            url = pinned_raw_url('https://github.com/'+slug,commit,filename)
            requests.setdefault(url, {'url':url,'purpose':'candidate official license or notice text at exact .cargo_vcs_info git.sha1','package':row['name'],'version':row['version'],'path':filename,'source_revision':commit,'repository_url':'https://github.com/'+slug,'path_in_vcs':relpath})
            requests[url].setdefault('owners',[]).append(binding_key)

    # Fetch exact raw files for every proven Cargo/npm source revision.
    pending = list(requests.values())
    # registry metadata already fetched; only raw requests lack HTTP response metadata
    for rec in pending:
        if rec.get('purpose','').startswith('exact npm'):
            continue
        if 'status' in rec or not rec.get('url','').startswith('https://raw.githubusercontent.com/'):
            continue
        body, net = http_get(rec['url'],budget,purpose=rec['purpose'])
        rec.update(net)
        if rec['status'] != 200:
            continue
        filename = rec['path']
        try:
            kind = text_kind(filename,body)
        except ValueError as e:
            rec['rejected_reason'] = str(e)
            continue
        sha = digest(body)
        out_rel = f'ThirdPartyLicenses/DependencyTexts/by-sha256/{sha}.txt'
        save(overlay_root/out_rel,body)
        rec['output_path'] = out_rel
        rec['kind'] = kind
        rec['bytes'] = len(body)
        for owner in rec.get('owners',[]):
            key = owner
            package_bindings.setdefault(key,{}).setdefault('files',[]).append({
                'source_url':rec['url'],'source_revision':rec.get('source_revision') or package_bindings.get(key,{}).get('registry_git_head'),
                'source_path':filename,'output_path':out_rel,'sha256':sha,'bytes':len(body),'kind':kind,
            })
        # npm raw requests are added below after metadata has yielded exact commits

    # Fetch npm files only after per-version registry metadata proves gitHead + repository + integrity.
    for name, binding in list(package_bindings.items()):
        if name not in NPM_FILES or binding.get('status') != 'exact_revision_verified':
            continue
        commit = binding.get('registry_git_head')
        repository_url = binding.get('repository_url')
        slug = github_slug(repository_url)
        if not slug or not commit:
            continue
        binding['files'] = []
        for filename in NPM_FILES[name]:
            url = pinned_raw_url(repository_url,commit,filename)
            body, rec = http_get(url,budget,purpose='npm license text at exact registry gitHead; registry dist.integrity matches package-lock')
            rec.update({'package':name,'version':next(r['version'] for r in idx['packages'] if r['ecosystem']=='npm' and r['name']==name),'path':filename,'source_revision':commit,'repository_url':'https://github.com/'+slug})
            requests[url] = rec
            if rec['status'] != 200:
                continue
            try:
                kind = text_kind(filename,body)
            except ValueError as e:
                rec['rejected_reason'] = str(e)
                continue
            sha = digest(body)
            out_rel = f'ThirdPartyLicenses/DependencyTexts/by-sha256/{sha}.txt'
            save(overlay_root/out_rel,body)
            rec['output_path'] = out_rel
            rec['kind'] = kind
            binding['files'].append({'source_url':url,'source_revision':commit,'source_path':filename,'output_path':out_rel,'sha256':sha,'bytes':len(body),'kind':kind})

    # The exact package source provides an SPDX expression or pointer to these canonical bodies;
    # keep the package pointer/identity separate from the canonical license source.
    canonical_sources = [
        ('selectors@0.36.1', 'MPL-2.0.txt', 'https://www.mozilla.org/media/MPL/2.0/index.815ca599c9df.txt', 'MPL-2.0'),
        ('siphasher@1.0.3', 'LICENSE-2.0.txt', 'https://www.apache.org/licenses/LICENSE-2.0.txt', 'Apache-2.0'),
        ('siphasher@1.0.3', 'MIT.txt', 'https://spdx.org/licenses/MIT.txt', 'MIT'),
    ]
    for binding_key, filename, url, expression_id in canonical_sources:
        binding = package_bindings.get(binding_key)
        if not binding or binding.get('status') != 'exact_revision_verified':
            continue
        body, rec = http_get(url,budget,purpose=f'official canonical {expression_id} license body identified by exact package license declaration/reference')
        rec.update({'package':binding_key.split('@',1)[0],'version':binding_key.rsplit('@',1)[1],'path':filename,'license_expression_id':expression_id,'package_source_revision':binding.get('source_revision')})
        requests[url] = rec
        if rec['status'] != 200:
            continue
        try:
            kind = text_kind(filename,body)
        except ValueError as e:
            rec['rejected_reason'] = str(e)
            continue
        sha = digest(body)
        out_rel = f'ThirdPartyLicenses/DependencyTexts/by-sha256/{sha}.txt'
        save(overlay_root/out_rel,body)
        rec['output_path'] = out_rel
        rec['kind'] = kind
        binding.setdefault('files',[]).append({'source_url':url,'source_revision':None,'package_source_revision':binding.get('source_revision'),'source_path':'canonical-license-texts/'+filename,'output_path':out_rel,'sha256':sha,'bytes':len(body),'kind':kind,'source_relationship':f'Canonical {expression_id} terms cited by the exact pinned package source; project notice/pointer remains separately indexed.'})

    attach_objc2_canonical_mit_texts(idx, package_bindings, overlay_root)

    # Persist exact network and binding evidence without local paths.
    for rec in requests.values():
        rec.pop('owners',None)
    prior_attempt=json.loads((evidence_root/'fetch-attempt-1.json').read_text()) if (evidence_root/'fetch-attempt-1.json').exists() else None
    prior_path_probe=json.loads((evidence_root/'github-path-probe.json').read_text()) if (evidence_root/'github-path-probe.json').exists() else None
    prior_bytes=(prior_attempt.get('network_body_bytes',0) if prior_attempt else 0)+(prior_path_probe.get('network_body_bytes',0) if prior_path_probe else 0)
    cumulative_bytes=prior_bytes+budget['bytes']
    if cumulative_bytes > CAP:
        raise RuntimeError('cumulative network body bytes exceed 4 MiB cap')
    receipt = {'artifact_type':'voiceflow-pinned-license-text-fetch-receipt','schema_version':1,'network_body_bytes':budget['bytes'],'cumulative_network_body_bytes':cumulative_bytes,'prior_network_evidence':{'fetch_attempt_1_sha256':digest((evidence_root/'fetch-attempt-1.json').read_bytes()) if prior_attempt else None,'github_path_probe_sha256':digest((evidence_root/'github-path-probe.json').read_bytes()) if prior_path_probe else None,'prior_network_body_bytes':prior_bytes},'network_body_cap_bytes':CAP,'requests':list(requests.values()),'source_bindings':package_bindings,'no_crate_archives_or_model_weights_requested':True,'no_credentials_used':True}
    receipt_bytes=(json.dumps(receipt,ensure_ascii=False,sort_keys=True,indent=2)+'\n').encode()
    save(evidence_root/'fetch-receipt.json',receipt_bytes)

    # Merge only source-proven full texts into a copy of the frozen input index.
    merged=json.loads(json.dumps(idx))
    merged=merge_pinned_text_bindings(merged,package_bindings,overlay_root)
    merged['summary']['network_license_fetch_body_bytes']=budget['bytes']
    merged['inputs']['pinned_upstream_fetch_receipt']={'path':'../../evidence/upstream-fetch/fetch-receipt.json','sha256':digest(receipt_bytes),'bytes':len(receipt_bytes)}
    save(index_path,(json.dumps(merged,ensure_ascii=False,sort_keys=True,indent=2)+'\n').encode())
    print(json.dumps({'network_body_bytes':budget['bytes'],'request_count':len(requests),'cargo_exact_bindings':sum(v.get('status')=='exact_revision_verified' for k,v in package_bindings.items() if not k.startswith('@') and k not in NPM_FILES and '@' in k),'npm_exact_bindings':sum(package_bindings.get(k,{}).get('status')=='exact_revision_verified' for k in NPM_FILES),'source_bound_text_files':sum(bool(v.get('files')) for v in package_bindings.values()),'summary':merged['summary']},sort_keys=True))
    return 0

if __name__=='__main__':
    raise SystemExit(main())
