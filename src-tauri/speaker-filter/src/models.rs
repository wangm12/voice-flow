use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use alkove_engine::Model;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

const UPSTREAM_REVISION: &str = "28fe044787952af5b6b6e59b3dd92de2b12eee36";
const MODEL_SPECS: [ModelSpec; 3] = [
    ModelSpec {
        key: "focus",
        name: "alkove-focus-1-preview",
        file: "alkove-focus-1-preview.alk",
        bytes: 1_111_696,
        sha256: "a63cda32e023b8d76089f4c3df0cc2c69ba36017b8f6cdea1da1094ef8022b6a",
    },
    ModelSpec {
        key: "cue",
        name: "alkove-cue-1-preview",
        file: "alkove-cue-1-preview.alk",
        bytes: 1_066_516,
        sha256: "977cd614996498f468536effbc2fb05138331751ca21572cf07b24184b4e7911",
    },
    ModelSpec {
        key: "timbre",
        name: "alkove-timbre-1-preview",
        file: "alkove-timbre-1-preview.alk",
        bytes: 739_632,
        sha256: "d498a7dd8b20528d783052b1e1d49e54ab7ee4e47aa12712d04f3d43edc68bd8",
    },
];

#[derive(Clone)]
pub struct LoadedModels {
    pub focus: Arc<Model>,
    pub cue: Arc<Model>,
    pub timbre: Arc<Model>,
    pub focus_id: String,
    pub cue_id: String,
    pub timbre_id: String,
    pub digest_sha256: String,
}

#[derive(Clone, Copy)]
struct ModelSpec {
    key: &'static str,
    name: &'static str,
    file: &'static str,
    bytes: usize,
    sha256: &'static str,
}

impl LoadedModels {
    pub fn load(models_dir: &Path) -> Result<Self> {
        if !models_dir.is_absolute() {
            return Err(Error::InvalidArgument(
                "--models must be an absolute directory",
            ));
        }
        let focus = load_one(models_dir, MODEL_SPECS[0])?;
        let cue = load_one(models_dir, MODEL_SPECS[1])?;
        let timbre = load_one(models_dir, MODEL_SPECS[2])?;

        let focus_id = model_id(MODEL_SPECS[0]);
        let cue_id = model_id(MODEL_SPECS[1]);
        let timbre_id = model_id(MODEL_SPECS[2]);
        let mut hash = Sha256::new();
        hash.update(b"voiceflow-speaker-filter-model-set-v1\0");
        for spec in MODEL_SPECS {
            hash.update(spec.key.as_bytes());
            hash.update([0]);
            hash.update(hex::decode(spec.sha256).expect("pinned digest is hex"));
        }
        let digest_sha256 = hex::encode(hash.finalize());

        Ok(Self {
            focus,
            cue,
            timbre,
            focus_id,
            cue_id,
            timbre_id,
            digest_sha256,
        })
    }

    pub fn require_profile_ids(
        &self,
        focus_id: &str,
        cue_id: &str,
        timbre_id: &str,
        models_digest: &str,
    ) -> Result<()> {
        if focus_id != self.focus_id
            || cue_id != self.cue_id
            || timbre_id != self.timbre_id
            || models_digest != self.digest_sha256
        {
            return Err(Error::Profile(
                "profile model identity does not match the pinned local models",
            ));
        }
        Ok(())
    }
}

pub fn upstream_revision() -> &'static str {
    UPSTREAM_REVISION
}

fn model_id(spec: ModelSpec) -> String {
    format!("{}@{}", spec.name, &spec.sha256[..12])
}

fn load_one(models_dir: &Path, spec: ModelSpec) -> Result<Arc<Model>> {
    let path = models_dir.join(spec.file);
    let metadata =
        std::fs::metadata(&path).map_err(|_| Error::Model("pinned model file is unavailable"))?;
    if metadata.len() != spec.bytes as u64 {
        return Err(Error::Model(
            "pinned model file has an incorrect byte length",
        ));
    }
    let file = std::fs::File::open(&path)
        .map_err(|_| Error::Model("pinned model file could not be read"))?;
    let mut bytes = Vec::with_capacity(spec.bytes);
    file.take(spec.bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Model("pinned model file could not be read"))?;
    if bytes.len() != spec.bytes {
        return Err(Error::Model("pinned model file changed while being read"));
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if digest != spec.sha256 {
        return Err(Error::Model("pinned model file has an incorrect SHA-256"));
    }
    let model = Model::load(&bytes).map_err(|_| Error::Model("pinned model file is invalid"))?;
    if model.name != spec.name {
        return Err(Error::Model("pinned model identity is incorrect"));
    }
    Ok(Arc::new(model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_model_set_digest_is_stable() {
        let mut hash = Sha256::new();
        hash.update(b"voiceflow-speaker-filter-model-set-v1\0");
        for spec in MODEL_SPECS {
            hash.update(spec.key.as_bytes());
            hash.update([0]);
            hash.update(hex::decode(spec.sha256).unwrap());
        }
        assert_eq!(
            hex::encode(hash.finalize()),
            "f5f4674894a155719d090f0dfc7532fd980d2ba055bf44e40cc44076c808c4d8"
        );
        assert_eq!(
            upstream_revision(),
            "28fe044787952af5b6b6e59b3dd92de2b12eee36"
        );
    }
}
