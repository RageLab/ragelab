use std::{
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub use ragelab_meta::MetaHash as YmapMetaHash;
pub use ragelab_ymap::{Quat as YmapQuat, Vec3 as YmapVec3, YmapEditCommand};
use ragelab_ymap::{Ymap, YmapEditSession, YmapEditTransactionResult};
use serde::Serialize;

pub const YMAP_AUTHORING_SCHEMA: &str = "ragelab.ymap.authoring";
pub const YMAP_AUTHORING_SCHEMA_VERSION: u64 = 1;

#[derive(Debug)]
pub enum YmapAuthoringError {
    InvalidSource(String),
    InvalidOutput(String),
    Conflict(String),
    Edit(String),
    Io(io::Error),
}

impl fmt::Display for YmapAuthoringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource(message) => {
                write!(formatter, "invalid YMAP authoring source: {message}")
            }
            Self::InvalidOutput(message) => {
                write!(formatter, "invalid YMAP authoring output: {message}")
            }
            Self::Conflict(message) => write!(formatter, "YMAP authoring conflict: {message}"),
            Self::Edit(message) => write!(formatter, "YMAP authoring edit failed: {message}"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl Error for YmapAuthoringError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for YmapAuthoringError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YmapAuthoringReport {
    pub schema: &'static str,
    pub schema_version: u64,
    pub source: PathBuf,
    pub saved_output: Option<PathBuf>,
    pub revision: u64,
    pub entities: usize,
    pub dirty: bool,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub source_conflict: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YmapAuthoringSaveResult {
    pub schema: &'static str,
    pub schema_version: u64,
    pub source: PathBuf,
    pub output: PathBuf,
    pub bytes_written: usize,
    pub entities: usize,
    pub revision: u64,
    pub semantic_reopen: bool,
    pub source_unchanged: bool,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct YmapAuthoringSession {
    source: PathBuf,
    source_snapshot: Vec<u8>,
    edit: YmapEditSession,
    saved_output: Option<PathBuf>,
}

impl YmapAuthoringSession {
    pub fn open(source: impl AsRef<Path>) -> Result<Self, YmapAuthoringError> {
        let source = source.as_ref();
        if !source.is_absolute() {
            return Err(YmapAuthoringError::InvalidSource(format!(
                "source must be an absolute path: {}",
                source.display()
            )));
        }
        let source = source.canonicalize()?;
        if source
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            != Some("ymap".into())
        {
            return Err(YmapAuthoringError::InvalidSource(format!(
                "source must have .ymap extension: {}",
                source.display()
            )));
        }
        let source_snapshot = fs::read(&source)?;
        Ymap::from_bytes(&source_snapshot)
            .map_err(|error| YmapAuthoringError::InvalidSource(error.to_string()))?;
        let edit = YmapEditSession::from_bytes(&source_snapshot)
            .map_err(|error| YmapAuthoringError::InvalidSource(error.to_string()))?;
        Ok(Self {
            source,
            source_snapshot,
            edit,
            saved_output: None,
        })
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn bytes(&self) -> &[u8] {
        self.edit.bytes()
    }

    pub fn document(&self) -> Result<Ymap, YmapAuthoringError> {
        self.edit
            .document()
            .map_err(|error| YmapAuthoringError::Edit(error.to_string()))
    }

    pub fn report(&self) -> Result<YmapAuthoringReport, YmapAuthoringError> {
        let document = self.document()?;
        Ok(YmapAuthoringReport {
            schema: YMAP_AUTHORING_SCHEMA,
            schema_version: YMAP_AUTHORING_SCHEMA_VERSION,
            source: self.source.clone(),
            saved_output: self.saved_output.clone(),
            revision: self.edit.revision(),
            entities: document.entities.len(),
            dirty: self.edit.dirty(),
            undo_depth: self.edit.undo_depth(),
            redo_depth: self.edit.redo_depth(),
            source_conflict: self.source_conflict()?,
        })
    }

    pub fn apply(
        &mut self,
        commands: &[YmapEditCommand],
    ) -> Result<YmapEditTransactionResult, YmapAuthoringError> {
        self.ensure_source_unchanged()?;
        self.edit
            .apply_transaction(commands)
            .map_err(|error| YmapAuthoringError::Edit(error.to_string()))
    }

    pub fn undo(&mut self) -> Result<YmapEditTransactionResult, YmapAuthoringError> {
        self.ensure_source_unchanged()?;
        self.edit
            .undo()
            .map_err(|error| YmapAuthoringError::Edit(error.to_string()))
    }

    pub fn redo(&mut self) -> Result<YmapEditTransactionResult, YmapAuthoringError> {
        self.ensure_source_unchanged()?;
        self.edit
            .redo()
            .map_err(|error| YmapAuthoringError::Edit(error.to_string()))
    }

    pub fn revert(&mut self) -> Result<YmapEditTransactionResult, YmapAuthoringError> {
        self.ensure_source_unchanged()?;
        self.edit
            .revert()
            .map_err(|error| YmapAuthoringError::Edit(error.to_string()))
    }

    pub fn source_conflict(&self) -> Result<bool, YmapAuthoringError> {
        Ok(fs::read(&self.source)? != self.source_snapshot)
    }

    pub fn save_as(
        &mut self,
        output: impl AsRef<Path>,
    ) -> Result<YmapAuthoringSaveResult, YmapAuthoringError> {
        self.ensure_source_unchanged()?;

        let output = output.as_ref();
        if !output.is_absolute() {
            return Err(YmapAuthoringError::InvalidOutput(format!(
                "output must be an absolute path: {}",
                output.display()
            )));
        }
        if output == self.source {
            return Err(YmapAuthoringError::InvalidOutput(
                "output must not overwrite the immutable source YMAP".into(),
            ));
        }
        if output.exists() {
            return Err(YmapAuthoringError::Conflict(format!(
                "output already exists; Save As never overwrites: {}",
                output.display()
            )));
        }
        if output
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            != Some("ymap".into())
        {
            return Err(YmapAuthoringError::InvalidOutput(format!(
                "output must have .ymap extension: {}",
                output.display()
            )));
        }

        let bytes = self.edit.bytes().to_vec();
        let expected = Ymap::from_bytes(&bytes)
            .map_err(|error| YmapAuthoringError::Edit(error.to_string()))?;

        if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        file.write_all(&bytes)?;
        file.flush()?;

        let written = fs::read(output)?;
        let reopened = Ymap::from_bytes(&written).map_err(|error| {
            YmapAuthoringError::Edit(format!("saved output failed semantic re-open: {error}"))
        })?;
        if reopened != expected {
            return Err(YmapAuthoringError::Edit(
                "saved output changed YMAP semantics after disk re-open".into(),
            ));
        }
        self.ensure_source_unchanged()?;

        self.edit.mark_saved();
        self.saved_output = Some(output.to_path_buf());

        Ok(YmapAuthoringSaveResult {
            schema: YMAP_AUTHORING_SCHEMA,
            schema_version: YMAP_AUTHORING_SCHEMA_VERSION,
            source: self.source.clone(),
            output: output.to_path_buf(),
            bytes_written: written.len(),
            entities: reopened.entities.len(),
            revision: self.edit.revision(),
            semantic_reopen: true,
            source_unchanged: true,
            dirty: self.edit.dirty(),
        })
    }

    fn ensure_source_unchanged(&self) -> Result<(), YmapAuthoringError> {
        if self.source_conflict()? {
            return Err(YmapAuthoringError::Conflict(format!(
                "source changed on disk since the session opened: {}",
                self.source.display()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ragelab_hash::jenkins;
    use ragelab_meta::MetaHash;

    use super::*;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/simple.ymap")
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ragelab-ymap-authoring-{label}-{}-{nonce}",
            process::id()
        ))
    }

    #[test]
    fn save_as_updates_dirty_baseline_and_revert_uses_last_saved_state() {
        let root = temp_root("save-revert");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.ymap");
        let output = root.join("saved.ymap");
        fs::copy(fixture(), &source).unwrap();
        let source_before = fs::read(&source).unwrap();

        let mut session = YmapAuthoringSession::open(&source).unwrap();
        session
            .apply(&[YmapEditCommand::SetProperties {
                index: 0,
                archetype_name: Some(MetaHash(jenkins("saved_archetype"))),
                flags: Some(123),
                parent_index: None,
            }])
            .unwrap();
        assert!(session.report().unwrap().dirty);

        let saved = session.save_as(&output).unwrap();
        assert!(saved.semantic_reopen);
        assert!(saved.source_unchanged);
        assert!(!saved.dirty);
        assert!(!session.report().unwrap().dirty);
        let saved_bytes = fs::read(&output).unwrap();

        session
            .apply(&[YmapEditCommand::SetProperties {
                index: 0,
                archetype_name: None,
                flags: Some(456),
                parent_index: None,
            }])
            .unwrap();
        assert!(session.report().unwrap().dirty);
        session.revert().unwrap();
        assert_eq!(session.bytes(), saved_bytes.as_slice());
        assert!(!session.report().unwrap().dirty);
        assert_eq!(fs::read(&source).unwrap(), source_before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_conflict_blocks_apply_and_save_without_overwrite() {
        let root = temp_root("conflict");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.ymap");
        let output = root.join("output.ymap");
        fs::copy(fixture(), &source).unwrap();

        let mut session = YmapAuthoringSession::open(&source).unwrap();
        let mut changed = fs::read(&source).unwrap();
        changed.push(0);
        fs::write(&source, changed).unwrap();

        assert!(session.report().unwrap().source_conflict);
        let error = session
            .apply(&[YmapEditCommand::SetProperties {
                index: 0,
                archetype_name: None,
                flags: Some(1),
                parent_index: None,
            }])
            .unwrap_err();
        assert!(matches!(error, YmapAuthoringError::Conflict(_)));
        let error = session.save_as(&output).unwrap_err();
        assert!(matches!(error, YmapAuthoringError::Conflict(_)));
        assert!(!output.exists());

        fs::remove_dir_all(root).unwrap();
    }
}
