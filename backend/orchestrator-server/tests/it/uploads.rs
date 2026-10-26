//! `POST /api/files` on the real workspace: an upload is stored untrusted under the team's
//! `uploads/`, never outside it and never over another upload, and a task that reads it is tainted.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::policy::TaintSource;
use orchestrator_server::tools::workspace::FILE_BYTES;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{Seeded, TempDir, agent_id, id_text, request, tainted, task_events};

const BOUNDARY: &str = "upload-boundary";

/// The seeded team and its files endpoint, `/api/files?team_id=<seeded team>`.
async fn served(pool: SqlitePool) -> (Seeded, String) {
    let seeded = Seeded::new(pool).await;
    let files = format!(
        "{}/api/files?team_id={}",
        seeded.serve().await,
        id_text(seeded.team)
    );
    (seeded, files)
}

/// One multipart field named `field` holding `bytes` as the file `name`.
fn body(field: &str, name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\nContent-Type: text/plain\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn upload(files: &str, body: Vec<u8>) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(files)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .send()
        .await
        .expect("response");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[sqlx::test]
async fn an_upload_is_stored_untrusted_under_uploads(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let (status, json) = upload(&base, body("file", "notes.md", b"# Notes\n")).await;
    assert_eq!(status, 201, "{json}");
    assert_eq!(json["files"][0]["path"], "uploads/notes.md");
    assert_eq!(json["files"][0]["bytes"], 8);
    assert_eq!(json["converted_from"], Value::Null, "text is kept as it is");
    assert_eq!(json["files"][0]["trust"], "untrusted");
    let on_disk =
        std::fs::read_to_string(served.temp.workspace().join(served.own("uploads/notes.md")))
            .expect("stored");
    assert_eq!(on_disk, "# Notes\n");
    let (trust, written_by_run): (String, Option<String>) =
        sqlx::query_as("SELECT trust, written_by_run FROM workspace_files WHERE path = ?")
            .bind(served.own("uploads/notes.md"))
            .fetch_one(&served.pool)
            .await
            .expect("row");
    assert_eq!((trust.as_str(), written_by_run), ("untrusted", None));

    let view: Value = reqwest::get(format!("{base}&path=uploads/notes.md"))
        .await
        .expect("response")
        .json()
        .await
        .expect("json");
    assert_eq!(view["trust"], "untrusted");
    assert!(view["written_by_root_run_id"].is_null());

    let (status, again) = upload(&base, body("file", "notes.md", b"replaced")).await;
    assert_eq!(
        (status, again["files"][0]["path"].clone()),
        (201, json!("uploads/notes-2.md")),
        "a taken name gets a number"
    );
    let kept =
        std::fs::read_to_string(served.temp.workspace().join(served.own("uploads/notes.md")))
            .expect("stored");
    assert_eq!(kept, "# Notes\n", "an upload never replaces one");
}

/// Any name is cleaned rather than refused, and any file is taken: text as it is, a binary file
/// as sent and marked unreadable. Only an oversized file is refused, before anything is written.
#[sqlx::test]
async fn any_name_and_any_file_are_taken_and_only_big_files_are_refused(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    for (raw, stored) in [
        ("../x.md", "uploads/x.md"),
        ("Raport Q3 (final).CSV", "uploads/Raport-Q3-final.CSV"),
        ("data.tsv", "uploads/data.tsv"),
        (".env", "uploads/env"),
    ] {
        let (status, json) = upload(&base, body("file", raw, b"x")).await;
        assert_eq!(
            (status, json["files"][0]["path"].clone()),
            (201, json!(stored)),
            "{raw:?}"
        );
    }
    assert!(
        !served.temp.workspace().join("x.md").exists(),
        "nothing outside the folder"
    );

    let (status, image) = upload(&base, body("file", "photo.png", b"PNG\0\0")).await;
    assert_eq!(
        (
            status,
            image["files"][0]["path"].clone(),
            image["files"][0]["readable"].clone()
        ),
        (201, json!("uploads/photo.png"), json!(false))
    );
    let (status, _) = upload(&base, body("other", "y.md", b"x")).await;
    assert_eq!(status, 422, "the field must be named file");
    let big = vec![b'a'; FILE_BYTES + 1];
    let (status, _) = upload(&base, body("file", "big.txt", &big)).await;
    assert_eq!(status, 413, "text over the read cap");
    let huge = vec![0_u8; 10 * 1024 * 1024 + 1];
    let (status, _) = upload(&base, body("file", "huge.bin", &huge)).await;
    assert_eq!(status, 413, "over 10 MB as sent");
    for refused in ["y.md", "big.txt", "huge.bin"] {
        let path = served
            .temp
            .workspace()
            .join(served.own(&format!("uploads/{refused}")));
        assert!(!path.exists(), "{refused} was stored");
    }
}

/// A two-sheet workbook becomes one CSV per sheet that `csv_query` can sum, and a Word document
/// becomes Markdown; both are untrusted like any upload.
#[sqlx::test]
async fn a_workbook_and_a_word_document_are_stored_as_text(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let (status, json) = upload(&base, body("file", "Budget Q3.xlsx", &xlsx())).await;
    assert_eq!(status, 201, "{json}");
    assert_eq!(json["converted_from"], "Budget-Q3.xlsx");
    let paths: Vec<&str> = json["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "uploads/Budget-Q3-Invoices.csv",
            "uploads/Budget-Q3-Notes.csv"
        ]
    );
    let invoices = std::fs::read_to_string(
        served
            .temp
            .workspace()
            .join(served.own("uploads/Budget-Q3-Invoices.csv")),
    )
    .expect("csv");
    assert_eq!(invoices, "invoice,amount\nINV-1,1200\nINV-2,16699.5\n");

    let (status, json) = upload(&base, body("file", "Policy.docx", &docx())).await;
    assert_eq!(
        (status, json["files"][0]["path"].clone()),
        (201, json!("uploads/Policy.md"))
    );
    let text = std::fs::read_to_string(
        served
            .temp
            .workspace()
            .join(served.own("uploads/Policy.md")),
    )
    .expect("md");
    assert_eq!(text, "# Payment policy\n\nOver 5k goes to the CFO.");

    let (status, broken) = upload(&base, body("file", "broken.xlsx", b"not a workbook")).await;
    assert_eq!(
        (status, broken["error"]["field"].clone()),
        (422, json!("file"))
    );
}

fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("entry");
        zip.write_all(content.as_bytes()).expect("write");
    }
    zip.finish().expect("zip").into_inner()
}

/// The smallest workbook Excel would open: two sheets of inline strings and numbers.
fn xlsx() -> Vec<u8> {
    let sheet = |rows: &str| {
        format!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
        )
    };
    let s = |r: &str, text: &str| format!(r#"<c r="{r}" t="inlineStr"><is><t>{text}</t></is></c>"#);
    let n = |r: &str, value: &str| format!(r#"<c r="{r}"><v>{value}</v></c>"#);
    let invoices = sheet(&format!(
        r#"<row r="1">{}{}</row><row r="2">{}{}</row><row r="3">{}{}</row>"#,
        s("A1", "invoice"),
        s("B1", "amount"),
        s("A2", "INV-1"),
        n("B2", "1200"),
        s("A3", "INV-2"),
        n("B3", "16699.5")
    ));
    let notes = sheet(&format!(r#"<row r="1">{}</row>"#, s("A1", "pay by Friday")));
    zip_of(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Invoices" sheetId="1" r:id="rId1"/><sheet name="Notes" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#,
        ),
        ("xl/worksheets/sheet1.xml", &invoices),
        ("xl/worksheets/sheet2.xml", &notes),
    ])
}

fn docx() -> Vec<u8> {
    zip_of(&[(
        "word/document.xml",
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Payment policy</w:t></w:r></w:p><w:p><w:r><w:t>Over 5k goes to the CFO.</w:t></w:r></w:p></w:body></w:document>"#,
    )])
}

#[cfg(unix)]
#[sqlx::test]
async fn a_symlinked_uploads_directory_is_refused(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let outside = TempDir::new();
    std::fs::create_dir_all(outside.workspace()).expect("dir");
    std::os::unix::fs::symlink(
        outside.workspace(),
        served.temp.team(served.team).join("uploads"),
    )
    .expect("symlink");
    let (status, json) = upload(&base, body("file", "notes.md", b"x")).await;
    assert_eq!(status, 422, "{json}");
    assert!(!outside.workspace().join("notes.md").exists());
}

#[cfg(unix)]
#[sqlx::test]
async fn an_upload_the_disk_refuses_is_a_server_error(pool: SqlitePool) {
    use std::os::unix::fs::PermissionsExt;
    let (served, base) = served(pool).await;
    let uploads = served.temp.team(served.team).join("uploads");
    std::fs::create_dir_all(&uploads).expect("dir");
    std::fs::set_permissions(&uploads, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let (status, json) = upload(&base, body("file", "notes.md", b"x")).await;
    std::fs::set_permissions(&uploads, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert_eq!(status, 500, "{json}");
    assert_eq!(json["error"]["code"], "internal");
}

/// An upload at the cap is stored and read back whole through the file tools' reader.
#[sqlx::test]
async fn an_upload_at_the_cap_is_readable(pool: SqlitePool) {
    let (_served, base) = served(pool).await;
    let full = vec![b'a'; FILE_BYTES];
    let (status, json) = upload(&base, body("file", "full.txt", &full)).await;
    assert_eq!(status, 201, "{json}");
    let view: Value = reqwest::get(format!("{base}&path=uploads/full.txt"))
        .await
        .expect("response")
        .json()
        .await
        .expect("json");
    assert_eq!(view["content"].as_str().map(str::len), Some(FILE_BYTES));
}

/// Every way a task can read an upload taints it with the file, as an inbox message would.
#[sqlx::test]
async fn reading_an_upload_taints_the_task(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let (status, _) = upload(
        &base,
        body("file", "people.csv", b"name,team\nAda,payroll\n"),
    )
    .await;
    assert_eq!(status, 201);
    let reader = agent_id(&served.pool, "reader").await;
    let upload_source = TaintSource::File {
        path: "uploads/people.csv".to_string(),
    };
    for script in [
        r#"test:call:file_read:{"path":"uploads/people.csv"}"#,
        r#"test:call:csv_query:{"path":"uploads/people.csv"}"#,
        r#"test:call:file_list:{"path":"uploads"}"#,
    ] {
        let mut rx = served.store.subscribe();
        let root = served
            .engine
            .start(StartRequest {
                agent_id: Some(reader),
                ..request(served.team, script)
            })
            .await
            .expect("start");
        let events = task_events(&mut rx, root).await;
        assert_eq!(
            tainted(&events),
            std::slice::from_ref(&upload_source),
            "{script}"
        );
    }
}
