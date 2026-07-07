use reqwest::StatusCode;
use std::process::{Child, Command};
use std::time::Duration;
use tempfile::TempDir;

struct TestServer {
    child: Child,
    pub _dir: TempDir, // kept alive for the duration of the test
    pub port: u16,
}

impl TestServer {
    fn start(port: u16) -> Self {
        let dir = TempDir::new().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_remote-fs-server"))
            .env("BASE_DIR", dir.path())
            .arg("--port")
            .arg(port.to_string())
            .spawn()
            .unwrap();

        std::thread::sleep(Duration::from_millis(500));
        Self {
            child,
            _dir: dir,
            port,
        }
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

#[tokio::test]
async fn test_mkdir_success() {
    let server = TestServer::start(3060);
    let client = reqwest::Client::new();

    let resp = client
        .post(&server.url("/mkdir/my_folder"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_put_and_get_file_success() {
    let server = TestServer::start(3061);
    let client = reqwest::Client::new();

    let resp = client
        .put(&server.url("/files/hello.txt"))
        .body("hello world")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .get(&server.url("/files/hello.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, "hello world");
}

#[tokio::test]
async fn test_get_nonexistent_file_returns_404() {
    let server = TestServer::start(3062);
    let client = reqwest::Client::new();

    let resp = client
        .get(&server.url("/files/does_not_exist.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_directory() {
    let server = TestServer::start(3063);
    let client = reqwest::Client::new();

    client
        .post(&server.url("/mkdir/nested"))
        .send()
        .await
        .unwrap();
    client
        .put(&server.url("/files/nested/file.txt"))
        .body("123")
        .send()
        .await
        .unwrap();

    let resp = client
        .get(&server.url("/list/nested"))
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
    let server = TestServer::start(3064);
    let client = reqwest::Client::new();

    client
        .put(&server.url("/files/old.txt"))
        .body("content")
        .send()
        .await
        .unwrap();

    let payload = serde_json::json!({ "new_name": "new.txt" });
    let resp = client
        .patch(&server.url("/meta/old.txt"))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Ensure old is gone
    let resp = client
        .get(&server.url("/files/old.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Ensure new exists
    let resp = client
        .get(&server.url("/files/new.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_file_success() {
    let server = TestServer::start(3065);
    let client = reqwest::Client::new();

    client
        .put(&server.url("/files/todelete.txt"))
        .body("content")
        .send()
        .await
        .unwrap();

    let resp = client
        .delete(&server.url("/files/todelete.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = client
        .get(&server.url("/files/todelete.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_path_traversal_returns_400() {
    let server = TestServer::start(3066);
    let client = reqwest::Client::new();

    let resp = client
        .get(&server.url("/files/..%2Fetc%2Fpasswd"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
