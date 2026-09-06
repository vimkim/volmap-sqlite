use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use tempfile::TempDir;
use volmap_sqlite::inspection::{InspectionError, InspectionSession, TextEncoding};

fn write_fixture(path: &Path, page_size: u32, page_count: u32, reserved_bytes: u8) {
    let mut header = [0_u8; 100];
    header[..16].copy_from_slice(b"SQLite format 3\0");
    let encoded_page_size = if page_size == 65_536 {
        1_u16
    } else {
        u16::try_from(page_size).expect("fixture page size fits")
    };
    header[16..18].copy_from_slice(&encoded_page_size.to_be_bytes());
    header[18] = 1;
    header[19] = 1;
    header[20] = reserved_bytes;
    header[21] = 64;
    header[22] = 32;
    header[23] = 32;
    header[28..32].copy_from_slice(&page_count.to_be_bytes());
    header[56..60].copy_from_slice(&1_u32.to_be_bytes());

    let mut file = File::create(path).expect("create fixture");
    file.write_all(&header).expect("write header");
    file.seek(SeekFrom::Start(
        u64::from(page_size) * u64::from(page_count) - 1,
    ))
    .expect("seek to fixture end");
    file.write_all(&[0]).expect("complete fixture");
}

#[test]
fn opens_a_valid_main_file_as_a_snapshot_scoped_page_graph() {
    let directory = TempDir::new().expect("temporary directory");
    let database_path = directory.path().join("catalog.sqlite");
    write_fixture(&database_path, 4_096, 3, 16);

    let session = InspectionSession::open(&database_path).expect("valid database");
    let graph = session.graph();

    assert_eq!(graph.snapshot.source.display_name, "catalog.sqlite");
    assert!(!graph.snapshot.source.id.is_empty());
    assert!(!graph.snapshot.id.is_empty());
    assert_eq!(graph.snapshot.geometry.page_size, 4_096);
    assert_eq!(graph.snapshot.geometry.usable_size, 4_080);
    assert_eq!(graph.snapshot.geometry.reserved_bytes, 16);
    assert_eq!(graph.snapshot.geometry.page_count, 3);
    assert_eq!(graph.snapshot.geometry.text_encoding, TextEncoding::Utf8);
    assert_eq!(
        graph
            .pages
            .iter()
            .map(|page| page.number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    let serialized = serde_json::to_string(graph).expect("serialize graph");
    assert!(!serialized.contains(directory.path().to_string_lossy().as_ref()));
}

#[test]
fn rejects_geometry_that_cannot_establish_complete_page_boundaries() {
    let directory = TempDir::new().expect("temporary directory");

    let short_path = directory.path().join("short.sqlite");
    File::create(&short_path)
        .expect("create fixture")
        .write_all(b"SQLite format 3\0")
        .expect("write fixture");
    assert!(matches!(
        InspectionSession::open(&short_path),
        Err(InspectionError::IncompleteHeader)
    ));

    let partial_page_path = directory.path().join("partial.sqlite");
    write_fixture(&partial_page_path, 4_096, 1, 0);
    let partial_page = File::options()
        .write(true)
        .open(&partial_page_path)
        .expect("open fixture");
    partial_page.set_len(4_095).expect("truncate fixture");
    assert!(matches!(
        InspectionSession::open(&partial_page_path),
        Err(InspectionError::MissingFirstPage)
    ));

    let trailing_bytes_path = directory.path().join("trailing.sqlite");
    write_fixture(&trailing_bytes_path, 4_096, 1, 0);
    let trailing_bytes = File::options()
        .append(true)
        .open(&trailing_bytes_path)
        .expect("open fixture");
    trailing_bytes.set_len(4_097).expect("extend fixture");
    assert!(matches!(
        InspectionSession::open(&trailing_bytes_path),
        Err(InspectionError::IncompletePage)
    ));
}

#[test]
fn rejects_non_sqlite_and_impossible_header_geometry() {
    let directory = TempDir::new().expect("temporary directory");

    let non_sqlite_path = directory.path().join("not-sqlite.db");
    write_fixture(&non_sqlite_path, 4_096, 1, 0);
    let mut non_sqlite = File::options()
        .write(true)
        .open(&non_sqlite_path)
        .expect("open fixture");
    non_sqlite
        .write_all(b"Not SQLite data")
        .expect("replace magic");
    assert!(matches!(
        InspectionSession::open(&non_sqlite_path),
        Err(InspectionError::InvalidMagic)
    ));

    let reserved_path = directory.path().join("reserved.sqlite");
    write_fixture(&reserved_path, 512, 1, 64);
    assert!(matches!(
        InspectionSession::open(&reserved_path),
        Err(InspectionError::InvalidReservedBytes(64))
    ));
}
