use reqwest::StatusCode;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct TestServer {
    child: Child,
    pub dir: TempDir,
    pub port: u16,
}

impl TestServer {
    fn start() -> Self {
        let port = free_port();
        let dir = TempDir::new().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_remote-fs-server"))
            .env("BASE_DIR", dir.path())
            .arg("--port")
            .arg(port.to_string())
            .spawn()
            .unwrap();

        assert!(wait_for_tcp(port, Duration::from_secs(5)), "server did not start");
        Self { child, dir, port }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_for_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

#[tokio::test]
async fn test_mkdir_success() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .post(server.url("/mkdir/my_folder"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_put_and_get_file_success() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .put(server.url("/files/hello.txt"))
        .body("hello world")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .get(server.url("/files/hello.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, "hello world");
}

#[tokio::test]
async fn test_get_nonexistent_file_returns_404() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .get(server.url("/files/does_not_exist.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_directory() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .post(server.url("/mkdir/nested"))
        .send()
        .await
        .unwrap();
    client
        .put(server.url("/files/nested/file.txt"))
        .body("123")
        .send()
        .await
        .unwrap();

    let resp = client
        .get(server.url("/list/nested"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json: serde_json::Value = resp.json().await.unwrap();
    let files = json.get("files").unwrap().as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].get("name").unwrap().as_str().unwrap(), "file.txt");
}

#[tokio::test]
async fn test_rename_file_success() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .put(server.url("/files/old.txt"))
        .body("content")
        .send()
        .await
        .unwrap();

    let payload = serde_json::json!({ "new_name": "new.txt" });
    let resp = client
        .patch(server.url("/meta/old.txt"))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(server.url("/files/old.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = client
        .get(server.url("/files/new.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_file_success() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .put(server.url("/files/todelete.txt"))
        .body("content")
        .send()
        .await
        .unwrap();

    let resp = client
        .delete(server.url("/files/todelete.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(server.url("/files/todelete.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_path_traversal_returns_400() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .get(server.url("/files/..%2Fetc%2Fpasswd"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_meta_for_file_and_directory() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .post(server.url("/mkdir/docs"))
        .send()
        .await
        .unwrap();
    client
        .put(server.url("/files/docs/readme.txt"))
        .body("metadata")
        .send()
        .await
        .unwrap();

    let file_meta: serde_json::Value = client
        .get(server.url("/meta/docs/readme.txt"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(file_meta["name"], "docs/readme.txt");
    assert_eq!(file_meta["is_dir"], false);
    assert_eq!(file_meta["size"], 8);

    let dir_meta: serde_json::Value = client
        .get(server.url("/meta/docs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(dir_meta["name"], "docs");
    assert_eq!(dir_meta["is_dir"], true);
}

#[tokio::test]
async fn test_patch_meta_updates_mode_and_timestamp() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .put(server.url("/files/attrs.txt"))
        .body("attrs")
        .send()
        .await
        .unwrap();

    let payload = serde_json::json!({
        "mode": 0o600,
        "mtime": 1_700_000_000_u64,
        "atime": 1_700_000_000_u64
    });
    let resp = client
        .patch(server.url("/meta/attrs.txt"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let meta: serde_json::Value = client
        .get(server.url("/meta/attrs.txt"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(meta["permissions"].as_u64().unwrap() & 0o777, 0o600);
    assert_eq!(meta["modified"].as_u64().unwrap(), 1_700_000_000);
}

#[tokio::test]
async fn test_get_file_range_variants() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .put(server.url("/files/range.txt"))
        .body("0123456789")
        .send()
        .await
        .unwrap();

    let resp = client
        .get(server.url("/files/range.txt"))
        .header(RANGE, "bytes=2-5")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(CONTENT_RANGE).unwrap(),
        "bytes 2-5/10"
    );
    assert_eq!(resp.text().await.unwrap(), "2345");

    let resp = client
        .get(server.url("/files/range.txt"))
        .header(RANGE, "bytes=7-")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.text().await.unwrap(), "789");

    let resp = client
        .get(server.url("/files/range.txt"))
        .header(RANGE, "bytes=99-100")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    let resp = client
        .get(server.url("/files/range.txt"))
        .header(RANGE, "items=0-1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_put_file_with_content_range_patches_existing_content() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    client
        .put(server.url("/files/patch.txt"))
        .body("hello world")
        .send()
        .await
        .unwrap();

    let resp = client
        .put(server.url("/files/patch.txt"))
        .header(CONTENT_RANGE, "bytes 6-10/11")
        .body("rust!")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let text = client
        .get(server.url("/files/patch.txt"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(text, "hello rust!");
}

#[tokio::test]
async fn test_put_file_with_empty_content_range_sets_final_size() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .put(server.url("/files/sparse.txt"))
        .header(CONTENT_RANGE, "bytes 4-7/8")
        .body("tail")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .put(server.url("/files/sparse.txt"))
        .header(CONTENT_RANGE, "bytes 0-0/8")
        .header(CONTENT_LENGTH, "0")
        .body("")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let meta: serde_json::Value = client
        .get(server.url("/meta/sparse.txt"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(meta["size"], 8);
}

#[tokio::test]
async fn test_negative_metadata_and_mutation_cases() {
    let server = TestServer::start();
    let client = reqwest::Client::new();

    let resp = client
        .get(server.url("/meta/missing.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = client
        .patch(server.url("/meta/missing.txt"))
        .json(&serde_json::json!({ "new_name": "renamed.txt" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = client
        .delete(server.url("/files/missing.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = client
        .put(server.url("/files/bad-range.txt"))
        .header(CONTENT_RANGE, "invalid")
        .body("data")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = client
        .patch(server.url("/meta/..%2Foutside.txt"))
        .json(&serde_json::json!({ "mode": 0o644 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore = "documents current symlink escape gap; enable after filer rejects symlink traversal"]
async fn test_symlink_escape_is_rejected() {
    let server = TestServer::start();
    let client = reqwest::Client::new();
    let outside = TempDir::new().unwrap();
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, b"secret").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), server.dir.path().join("outside_link")).unwrap();

    let resp = client
        .get(server.url("/files/outside_link/secret.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
