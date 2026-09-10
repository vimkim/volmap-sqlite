#[path = "../benchmarks/probe.rs"]
mod probe;

#[test]
#[ignore = "process worker, invoked by the constrained-memory regression"]
fn profile_worker() {
    let path = std::env::var_os("VOLMAP_PROFILE_INPUT").expect("worker input");
    let ceiling = if std::env::var_os("VOLMAP_PROFILE_TIGHT").is_some() {
        // Establish runtime/code residency before granting a small allocation allowance.
        // The parent creates this fixture, as it does the measured input.
        let warmup = std::env::var_os("VOLMAP_PROFILE_WARMUP").unwrap();
        let session =
            volmap_sqlite::inspection::InspectionSession::begin(std::path::Path::new(&warmup))
                .unwrap()
                .with_storage_budget(volmap_sqlite::inspection::StorageBudget {
                    cache_bytes: 0,
                    ..Default::default()
                });
        session
            .scan(|_| volmap_sqlite::inspection::ScanControl::Continue)
            .unwrap();
        session.graph().unwrap();
        drop(session);
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let resident: u64 = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        resident * 1024 + 3 * 1024 * 1024
    } else {
        64 * 1024 * 1024
    };
    let result = probe::run(std::path::Path::new(&path), ceiling, 64 * 1024).unwrap();
    println!("PROFILE_RESULT={result}");
}

#[test]
fn high_degree_diagnostics_stop_before_allocating_beyond_the_resident_ceiling() {
    use std::io::{Seek, SeekFrom, Write};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("many-parents.sqlite");
    let database = rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch("PRAGMA page_size=512; CREATE TABLE entries(value);")
        .unwrap();
    drop(database);
    let warmup = directory.path().join("warmup.sqlite");
    std::fs::copy(&path, &warmup).unwrap();
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(28)).unwrap();
    file.write_all(&2002_u32.to_be_bytes()).unwrap();
    file.seek(SeekFrom::Start(1024)).unwrap();
    let mut parent = [0_u8; 512];
    parent[0] = 5;
    parent[5..7].copy_from_slice(&512_u16.to_be_bytes());
    parent[8..12].copy_from_slice(&2_u32.to_be_bytes());
    for _ in 0..2000 {
        file.write_all(&parent).unwrap();
    }
    drop(file);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "profile_worker", "--ignored", "--nocapture"])
        .env("VOLMAP_PROFILE_INPUT", &path)
        .env("VOLMAP_PROFILE_TIGHT", "1")
        .env("VOLMAP_PROFILE_WARMUP", &warmup)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let result: serde_json::Value = serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("PROFILE_RESULT="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["evaluatedPages"], 2002);
    assert_eq!(result["summary"]["coverage"]["reason"], "complete");
    assert_eq!(result["summary"]["topologyCoverage"]["reason"], "budget");
    assert_eq!(
        result["summary"]["topologyCoverage"]["phase"],
        "btree_parent_reconciliation"
    );
    assert_eq!(result["summary"]["diagnosticCount"], 0);
    let last = result["workCoverage"].as_array().unwrap().last().unwrap();
    assert_eq!(last["limit"], "resident_memory");
    assert_eq!(
        last["next"].as_u64().unwrap(),
        last["evaluated"].as_u64().unwrap() + 1
    );
    assert!(
        result["peakResidentBytes"].as_u64().unwrap()
            <= result["operationalBudget"]["maxResidentBytes"]
                .as_u64()
                .unwrap(),
        "{result}"
    );
}

#[test]
fn constrained_memory_inspection_remains_unsampled_as_the_input_grows() {
    use std::io::{Seek, SeekFrom, Write};
    let directory = tempfile::tempdir().unwrap();
    let mut measurements = Vec::new();
    for size_mib in [8_u64, 32] {
        let path = directory.path().join(format!("sparse-{size_mib}.sqlite"));
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute_batch("PRAGMA page_size=65536; CREATE TABLE entries(value);")
            .unwrap();
        drop(database);
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(size_mib * 1024 * 1024).unwrap();
        file.seek(SeekFrom::Start(28)).unwrap();
        file.write_all(&u32::try_from(size_mib * 16).unwrap().to_be_bytes())
            .unwrap();
        drop(file);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "profile_worker", "--ignored", "--nocapture"])
            .env("VOLMAP_PROFILE_INPUT", &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let result: serde_json::Value = serde_json::from_str(
            stdout
                .lines()
                .find_map(|line| line.strip_prefix("PROFILE_RESULT="))
                .expect("worker measurement"),
        )
        .unwrap();
        assert_eq!(result["evaluatedPages"], size_mib * 16);
        assert_eq!(result["summary"]["coverage"]["reason"], "complete");
        assert_eq!(result["summary"]["topologyCoverage"]["reason"], "complete");
        assert_eq!(result["firstPage"], 1);
        assert_eq!(result["lastPage"], size_mib * 16);
        assert!(result["storage"]["spilledIndexes"].as_u64().unwrap() > 0);
        assert!(result["storage"]["cacheBytes"].as_u64().unwrap() <= 64 * 1024);
        let peak = result["peakResidentBytes"].as_u64().unwrap();
        assert!(peak < 64 * 1024 * 1024, "{result}");
        measurements.push(peak);
    }
    assert!(
        measurements[1] <= measurements[0] + 8 * 1024 * 1024,
        "input grew fourfold, peaks: {measurements:?}"
    );
}
