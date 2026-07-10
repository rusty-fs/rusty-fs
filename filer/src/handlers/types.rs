use serde::Serialize;

#[derive(Serialize)]
pub(super) struct FileEntry {
    pub(super) name: String,
    pub(super) is_dir: bool,
    pub(super) size: u64,
    pub(super) modified: Option<u64>,
    pub(super) permissions: Option<u32>,
}
