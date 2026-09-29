#![allow(unused_variables, dead_code, unused_imports)]
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
pub(crate) const FOLDER_LIMIT: usize = 1000;
/// Go: type folderEntry struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FolderEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) checkout: bool,
    pub(crate) registered: String,
}
/// Go: type folderRoot struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FolderRoot {
    pub(crate) label: String,
    pub(crate) path: String,
}
/// Go: type folderListing struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FolderListing {
    pub(crate) path: String,
    pub(crate) parent: String,
    pub(crate) home: String,
    pub(crate) roots: Vec<serde_json::Value>,
    pub(crate) checkout: bool,
    pub(crate) registered: String,
    pub(crate) inside: String,
    pub(crate) entries: Vec<serde_json::Value>,
    pub(crate) truncated: bool,
}
impl Handler {
    /// Go: func (h *Handler) additionFolders(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_folders(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::folders::addition_folders")
    }
}
/// Go: func loopbackHost(r *http.Request) bool
pub(crate) fn loopback_host(r: &crate::httpx::Request) -> bool {
    todo!("hub::folders::loopback_host")
}
/// Go: func insideProject(registered map[string]string, path string) string
pub(crate) fn inside_project(registered: serde_json::Value, path: String) -> String {
    todo!("hub::folders::inside_project")
}
/// Go: func isCheckout(dir string) bool
pub(crate) fn is_checkout(dir: String) -> bool {
    todo!("hub::folders::is_checkout")
}
