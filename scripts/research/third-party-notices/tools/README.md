# Offline dependency notice generator

`generate_notices.py` requires Python 3.11 or later and reads a VoiceFlow repository, a matching Cargo metadata JSON file, and the reviewed lock-graph observations. It writes an additive `project-overlay/` tree and a separate source/output manifest. The output is an attribution inventory, not legal clearance or a proof that each listed package is linked into the application.

Example invocation:

```sh
python3 tools/generate_notices.py \
  --repo <voice-flow-repository> \
  --cargo-metadata <offline-cargo-metadata.json> \
  --observations <reviewed-lock-observations.json> \
  --output-root <new-external-stage>/project-overlay \
  --evidence-root <new-external-stage>/evidence
```

The two output roots must not exist yet. Their parents must already exist, be real directories rather than symlinks, and be outside the repository and source inputs. The roots must be distinct and may not contain or overlap each other. The generator checks the paths before creating either root, rejects symlink path components, and constrains every file write to its declared output/evidence root. It does not overwrite prior output or evidence.

`--repo`, Cargo metadata, and observations are read-only inputs. Existing repository `ThirdPartyLicenses/` texts and notice baselines are included as source identities in the receipt; generated files are written only into the external overlay and evidence roots. Lock identity mismatches stop the run. File hashes and byte counts in the manifest let reviewers compare each discovered text with its saved copy.

## Optional pinned-source text fetch

`fetch_verified_sources.py` is a separate network step for candidate license and notice text only. It verifies Cargo source revisions from each package's `.cargo_vcs_info.json`; npm metadata must match the lockfile's exact version and integrity and provide an exact `gitHead` from the expected repository before any raw source path is requested. A package version is never treated as a Git tag. The fetcher caps cumulative response bodies at 4 MiB and individual responses at 160 KiB. It does not request crate archives, package tarballs, model weights, or binaries. An integrity or source-identity mismatch leaves the row unresolved and prevents the pinned raw-text request.

The fetcher CLI accepts explicit `--repo`, `--inventory-index`, `--cargo-metadata`, `--overlay-root`, and `--evidence-root` paths. Review the generated `fetch-receipt.json` and source bindings before carrying any newly saved text into an output inventory. The checked-in mocked-HTTP tests exercise the CLI path without live network access, including an integrity mismatch that stops before the raw-source request.

## Reusable offline tests

From this tests directory, run `python3 -B -m unittest test_generate_notices test_fetch_verified_sources -v`. The real-text classifier cases resolve `ThirdPartyLicenses/DependencyTexts` from the repository root; they do not expect a candidate-local `project-overlay`. The reclassification/objc2 binding test uses a self-contained temporary fixture. Historical candidate replay remains in its private evidence workflow. The two modules use only Python's standard library and do not need Cargo, npm dependencies, network access, or an installed package tree.
