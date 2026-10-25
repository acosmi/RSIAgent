use std::{fs, io::Read, path::Path};

use flate2::{Crc, Decompress, FlushDecompress, Status, bufread::DeflateDecoder};

const RAW_HELLO: &[u8] = &[0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];

#[test]
fn raw_deflate_known_answer_respects_output_bounds() {
    let mut decoder = Decompress::new(false);
    let mut output = [0; 6];
    let status = decoder
        .decompress(RAW_HELLO, &mut output, FlushDecompress::Finish)
        .expect("decode the raw DEFLATE known answer");
    assert_eq!(status, Status::StreamEnd);
    assert_eq!(decoder.total_in(), 7);
    assert_eq!(decoder.total_out(), 5);
    assert_eq!(&output[..5], b"hello");

    // A size-plus-one buffer lets a caller detect an exceeded output limit.
    let mut bounded_decoder = Decompress::new(false);
    let mut bounded_output = [0; 3];
    let bounded_status = bounded_decoder
        .decompress(RAW_HELLO, &mut bounded_output, FlushDecompress::Finish)
        .expect("decode only as far as the supplied buffer permits");
    assert_ne!(bounded_status, Status::StreamEnd);
    assert_eq!(bounded_decoder.total_out(), 3);
    assert_eq!(&bounded_output, b"hel");

    let mut reader = DeflateDecoder::new(RAW_HELLO).take(3);
    let mut read_output = Vec::new();
    assert_eq!(reader.read_to_end(&mut read_output).unwrap(), 3);
    assert_eq!(read_output, b"hel");
}

#[test]
fn crc32_matches_standard_check_value() {
    let mut crc = Crc::new();
    crc.update(b"123456789");
    assert_eq!(crc.sum(), 0xcbf4_3926);
}

fn locked_package<'a>(lock: &'a str, name: &str) -> &'a str {
    let name_line = format!("name = \"{name}\"");
    let packages: Vec<_> = lock
        .split("[[package]]")
        .filter(|package| package.lines().any(|line| line == name_line))
        .collect();
    assert_eq!(packages.len(), 1, "one locked identity for {name}");
    packages[0]
}

#[test]
fn registered_backend_matches_locked_dependency() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock = fs::read_to_string(manifest_dir.join("../../Cargo.lock")).unwrap();
    for (name, version, checksum) in [
        (
            "flate2",
            "1.1.10",
            "6e634e2e0ebac1ee034020da1ca582e17ffe4e0f5e985823721e168928136dcb",
        ),
        (
            "zlib-rs",
            "0.6.8",
            "b268e58e7c693d7c271f93ffc4ba3b380412554231c85bf61ca7af91042a4112",
        ),
    ] {
        let package = locked_package(&lock, name);
        assert!(
            package
                .lines()
                .any(|line| line == format!("version = \"{version}\""))
        );
        assert!(
            package
                .lines()
                .any(|line| line == format!("checksum = \"{checksum}\""))
        );
    }
    for forbidden in [
        "miniz_oxide",
        "adler2",
        "crc32fast",
        "simd-adler32",
        "zopfli",
        "zip",
    ] {
        assert!(
            !lock
                .lines()
                .any(|line| line == format!("name = \"{forbidden}\"")),
            "unexpected backend package: {forbidden}"
        );
    }

    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml")).unwrap();
    let declaration =
        r#"flate2 = { version = "=1.1.10", default-features = false, features = ["zlib-rs"] }"#;
    assert_eq!(
        manifest.lines().filter(|line| *line == declaration).count(),
        1
    );

    // The register uses canonical JSON; isolate its top-level package record.
    let register =
        fs::read_to_string(manifest_dir.join("../../docs/dependency-register.json")).unwrap();
    let records: Vec<_> = register
        .split("\n    {\n")
        .filter(|record| {
            record
                .lines()
                .any(|line| line == r#"      "name": "flate2","#)
        })
        .collect();
    assert_eq!(records.len(), 1);
    let record = records[0];
    assert!(record.contains(r#"      "version": "1.1.10""#));
    assert!(record.contains(r#"      "license": "MIT OR Apache-2.0""#));
    let compact: String = record.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(compact.contains(concat!(
        r#""consumers":[{"alias":"flate2","declaration":{"default-features":false,"features":["zlib-rs"],"version":"=1.1.10"},"#,
        r#""default_features":false,"features":["zlib-rs"],"kind":"regular","manifest":"crates/evo-engine/Cargo.toml","target":null,"workspace_declaration":null}]"#,
    )));
}
