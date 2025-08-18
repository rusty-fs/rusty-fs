use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, 
    ReplyEntry, Request, FUSE_ROOT_ID,
};
use libc::ENOENT;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::raw::mode_t;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use url::{Url, ParseError};

const TTL: Duration = Duration::from_secs(1);

// Strutture per comunicazione con il server
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>, // Unix timestamp in seconds
    permissions: Option<mode_t>, // File permissions
}


#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirectoryListing {
    files: Vec<FileEntry>,
    message: String,
}

// HTTP Client per comunicare con il server remoto
#[derive(Clone)]
struct HttpClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpClient {
    fn new(base_url: String) -> Self {
        // Ensure the base_url has the protocol
        let base_url = if base_url.starts_with("http://") || base_url.starts_with("https://") {
            base_url
        } else {
            format!("http://{}", base_url)
        };
        
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    async fn list_directory(&self, path: &str) -> anyhow::Result<Vec<FileEntry>> {
        let url = format!("{}/list{}", self.base_url, path);
        println!("GET {}", url);
         
        let response = self.client.get(&url).send().await;

        
        match response {
            Ok(response) => {
                // Get the response text first to debug
                let response_text = response.text().await?;
                
                // Check if response is empty
                if response_text.trim().is_empty() {
                    return Err(anyhow::anyhow!("Empty response from server"));
                }
                
                // Parse the JSON as DirectoryListing and extract the files array
                let listing: DirectoryListing = serde_json::from_str(&response_text)
                    .map_err(|e| anyhow::anyhow!("JSON parse error: {} - Response: {}", e, response_text))?;
                println!("Response body: {:?}", listing.files);
  
                Ok(listing.files)
            }            
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to send request: {}", e));
            }
        }
        
    }
}

// Il filesystem principale
pub struct RemoteFileSystem {
    http_client: HttpClient,
    runtime: Runtime, // Change from Handle to Runtime
    // Mapping tra inode e path
    inode_to_path: HashMap<u64, String>,
    path_to_inode: HashMap<String, u64>,
    next_inode: u64,
}

impl RemoteFileSystem {
    pub fn new(server_url: String) -> Self {
        let mut fs = Self {
            http_client: HttpClient::new(server_url),
            runtime: Runtime::new().unwrap(), // Create new runtime
            inode_to_path: HashMap::new(),
            path_to_inode: HashMap::new(),
            next_inode: 2, // 1 è riservato per root
        };

        // Registra la root directory
        fs.inode_to_path.insert(FUSE_ROOT_ID, "/".to_string());
        fs.path_to_inode.insert("/".to_string(), FUSE_ROOT_ID);

        fs
    }

    fn get_inode_for_path(&mut self, path: &str) -> u64 {
        if let Some(&inode) = self.path_to_inode.get(path) {
            return inode;
        }

        let inode = self.next_inode;
        self.next_inode += 1;
        self.inode_to_path.insert(inode, path.to_string());
        self.path_to_inode.insert(path.to_string(), inode);
        inode
    }

    fn get_path_for_inode(&self, inode: u64) -> Option<String> {
        self.inode_to_path.get(&inode).cloned()
    }

    fn file_entry_to_attr(&self, entry: &FileEntry, inode: u64) -> FileAttr {
        
        let modified = UNIX_EPOCH + Duration::from_secs(entry.modified.unwrap_or(0));
        
        FileAttr {
            ino: inode,
            size: entry.size,
            blocks: (entry.size + 511) / 512,
            atime: modified,
            mtime: modified,
            ctime: modified,
            crtime: modified,
            kind: if entry.is_dir { FileType::Directory } else { FileType::RegularFile },
            perm: if entry.is_dir { 0o755 } else { 0o644 },
            nlink: 1,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
}



impl Filesystem for RemoteFileSystem {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        println!("lookup: parent={}, name={:?}", parent, name);

        let parent_path = match self.get_path_for_inode(parent) {
            Some(path) => path,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        // Costruisci il path completo
        let full_path = if parent_path == "/" {
            format!("/{}", name_str)
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let client = self.http_client.clone();
        let full_path_clone = full_path.clone();
        let result = self.runtime.block_on(async move {
            
            if let Ok(entries) = client.list_directory(&full_path_clone).await {
                // Stampa le entry per debug
                println!("Entries found: {:?}", entries);

                if let Some(entry) = entries.iter().find(|e| e.name == name_str) {
                    return Ok(entry.clone());
                }
            }
     
            Err(anyhow::anyhow!("File not found"))
        });

        match result {
            Ok(entry) => {
                let inode = self.get_inode_for_path(&full_path);
                let attr = self.file_entry_to_attr(&entry, inode);
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                println!("lookup failed: {}", e);
                reply.error(ENOENT);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        println!("getattr: ino={}", ino);
        
        let path = match self.get_path_for_inode(ino) {
            Some(path) => path,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let client = self.http_client.clone();
        let result = self.runtime.block_on(async move {
            // Per la root directory
            if path == "/" {
                return Ok(FileEntry {
                    name: "".to_string(),
                    is_dir: true,
                    size: 0,
                    modified: Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()),
                    permissions: None,
                });
            }
            
            // Prova come directory
            if let Ok(_) = client.list_directory(&path).await {
                return Ok(FileEntry {
                    name: path.split('/').last().unwrap_or("").to_string(),
                    is_dir: true,
                    size: 0,
                    modified: Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()),
                    permissions: None,
                });
            }
            
            // Fallback per file (se implementi get_file_metadata)
            Err(anyhow::anyhow!("Not found"))
        });

        match result {
            Ok(entry) => {
                let attr = self.file_entry_to_attr(&entry, ino);
                reply.attr(&TTL, &attr);
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        println!("readdir: ino={}, offset={}", ino, offset);
        
        let path = match self.get_path_for_inode(ino) {
            Some(path) => path,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let path_clone = path.clone();
        let client = self.http_client.clone();
        let result = self.runtime.block_on(async move {
            client.list_directory(&path_clone).await
        });

        match result {
            Ok(entries) => {
                let mut current_offset = 1i64;
                
                // Aggiungi le directory speciali "." e ".."
                if offset < current_offset {
                    if reply.add(ino, current_offset, FileType::Directory, ".") {
                        reply.ok();
                        return;
                    }
                }
                current_offset += 1;
                
                if offset < current_offset {
                    let parent_ino = if ino == FUSE_ROOT_ID { FUSE_ROOT_ID } else { FUSE_ROOT_ID };
                    if reply.add(parent_ino, current_offset, FileType::Directory, "..") {
                        reply.ok();
                        return;
                    }
                }
                current_offset += 1;

                // Aggiungi le entry reali
                for entry in entries.iter().skip((offset - 2).max(0) as usize) {
                    let entry_path = if path == "/" {
                        format!("/{}", entry.name)
                    } else {
                        format!("{}/{}", path, entry.name)
                    };
                    
                    let entry_inode = self.get_inode_for_path(&entry_path);
                    let file_type = if entry.is_dir { FileType::Directory } else { FileType::RegularFile };
                    
                    if reply.add(entry_inode, current_offset, file_type, &entry.name) {
                        break;
                    }
                    current_offset += 1;
                }
                reply.ok();
            }
            Err(e) => {
                println!("readdir failed: {}", e);
                reply.error(ENOENT);
            }
        }
    }
}

fn main() -> anyhow::Result<()> { 
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <server_url> <mountpoint>", args[0]);
        std::process::exit(1);
    }

    let server_url = &args[1];
    let mountpoint = &args[2];

    println!("Mounting remote filesystem from {} to {}", server_url, mountpoint);

    let fs = RemoteFileSystem::new(server_url.to_string());
    
    let options = vec![
        MountOption::RO,               // Read-only
        MountOption::FSName("remote-fs".to_string()),
    ];

    fuser::mount2(fs, mountpoint, &options)?;
    
    Ok(())
}