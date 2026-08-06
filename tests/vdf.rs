use steamtrain::vdf::{self, Value};

const SAMPLE: &[u8] =
    b"\"Root\"\n{\n\t\"key\"\t\t\"value\"\n\t\"Nested\"\n\t{\n\t\t\"a\"\t\t\"1\"\n\t}\n}\n";

#[test]
fn parses_nested_blocks() {
    let d = vdf::loads(SAMPLE).unwrap();
    let root = d.get_block(b"Root").unwrap();
    assert_eq!(root.get_str(b"key"), Some(&b"value"[..]));
    assert_eq!(
        root.get_block(b"Nested").unwrap().get_str(b"a"),
        Some(&b"1"[..])
    );
}

#[test]
fn round_trip_is_byte_identical() {
    assert_eq!(vdf::dumps(&vdf::loads(SAMPLE).unwrap()), SAMPLE);
}

#[test]
fn escapes_survive_a_round_trip() {
    let mut inner = vdf::Block::new();
    inner.insert(b"k".to_vec(), Value::Str(br#"a "quoted" \ value"#.to_vec()));
    let mut root = vdf::Block::new();
    root.insert(b"R".to_vec(), Value::Block(inner));

    let text = vdf::dumps(&root);
    let parsed = vdf::loads(&text).unwrap();
    assert_eq!(
        parsed.get_block(b"R").unwrap().get_str(b"k"),
        Some(&br#"a "quoted" \ value"#[..])
    );
}

#[test]
fn tabs_and_newlines_in_a_value_survive_a_round_trip() {
    let mut inner = vdf::Block::new();
    inner.insert(b"k".to_vec(), Value::Str(b"one\ttwo\nthree".to_vec()));
    let mut root = vdf::Block::new();
    root.insert(b"R".to_vec(), Value::Block(inner));

    let text = vdf::dumps(&root);
    // The escapes must be written, not the raw control bytes, or the file
    // would no longer parse as one record per line.
    assert!(String::from_utf8(text.clone())
        .unwrap()
        .contains(r"one\ttwo\nthree"));
    let parsed = vdf::loads(&text).unwrap();
    assert_eq!(
        parsed.get_block(b"R").unwrap().get_str(b"k"),
        Some(&b"one\ttwo\nthree"[..])
    );
}

#[test]
fn skips_line_comments() {
    let d = vdf::loads(b"// comment\n\"R\"\n{\n\t\"k\"\t\t\"v\"\n}\n").unwrap();
    assert_eq!(d.get_block(b"R").unwrap().get_str(b"k"), Some(&b"v"[..]));
}

#[test]
fn parses_an_empty_block() {
    let d = vdf::loads(b"\"R\"\n{\n\t\"apps\"\n\t{\n\t}\n}\n").unwrap();
    assert!(d
        .get_block(b"R")
        .unwrap()
        .get_block(b"apps")
        .unwrap()
        .is_empty());
}

#[test]
fn preserves_key_order() {
    let d = vdf::loads(b"\"R\"\n{\n\t\"b\"\t\t\"1\"\n\t\"a\"\t\t\"2\"\n}\n").unwrap();
    let keys: Vec<&[u8]> = d.get_block(b"R").unwrap().keys().collect();
    assert_eq!(keys, vec![&b"b"[..], &b"a"[..]]);
}

#[test]
fn skips_platform_conditionals() {
    let d = vdf::loads(b"\"R\"\n{\n\t\"k\" [$LINUX]\t\t\"v\"\n}\n").unwrap();
    assert_eq!(d.get_block(b"R").unwrap().get_str(b"k"), Some(&b"v"[..]));
}

#[test]
fn accepts_bare_unquoted_tokens() {
    let d = vdf::loads(b"R\n{\n\tkey\tvalue\n}\n").unwrap();
    assert_eq!(
        d.get_block(b"R").unwrap().get_str(b"key"),
        Some(&b"value"[..])
    );
}

#[test]
fn a_block_keeps_its_position_among_its_siblings() {
    let source =
        b"\"R\"\n{\n\t\"a\"\t\t\"1\"\n\t\"B\"\n\t{\n\t\t\"x\"\t\t\"1\"\n\t}\n\t\"c\"\t\t\"2\"\n}\n";
    let parsed = vdf::loads(source).unwrap();
    let keys: Vec<&[u8]> = parsed.get_block(b"R").unwrap().keys().collect();
    assert_eq!(keys, vec![&b"a"[..], &b"B"[..], &b"c"[..]]);
    assert_eq!(vdf::dumps(&parsed), source);
}

#[test]
fn invalid_utf8_in_a_value_round_trips_unchanged() {
    // A game name in a broken encoding must come back out byte-for-byte. This
    // is the case Python covered with errors="surrogateescape", and the one
    // String::from_utf8_lossy would silently corrupt.
    let mut source = Vec::new();
    source.extend_from_slice(b"\"R\"\n{\n\t\"name\"\t\t\"caf");
    source.push(0xE9); // latin-1 e-acute: not valid UTF-8
    source.extend_from_slice(b"\"\n}\n");

    assert_eq!(vdf::dumps(&vdf::loads(&source).unwrap()), source);
}

#[test]
fn rejects_malformed_input() {
    assert!(vdf::loads(b"\"unterminated").is_err());
    assert!(vdf::loads(b"\"R\"\n{\n").is_err());
    assert!(vdf::loads(b"}").is_err());
    assert!(vdf::loads(b"\"R\"\n{\n\t\"dangling\"\n}\n").is_err());
    assert!(vdf::loads(b"{\n}\n").is_err());
    assert!(vdf::loads(b"\"R\" [$LINUX").is_err());
}

#[test]
fn real_steam_files_round_trip() {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let home = std::path::PathBuf::from(home);

    // Every root the Core itself looks in, not just the most common one. The
    // Python equivalent of this test only checked ~/.local/share/Steam and so
    // silently skipped on any machine using a different layout - including a
    // stock Debian/Ubuntu install, where the real root is ~/.steam/steam.
    let roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ];

    let mut candidates = Vec::new();
    for steam in roots {
        if let Ok(users) = std::fs::read_dir(steam.join("userdata")) {
            for entry in users.flatten() {
                candidates.push(entry.path().join("config/localconfig.vdf"));
            }
        }
        if let Ok(apps) = std::fs::read_dir(steam.join("steamapps")) {
            for entry in apps.flatten() {
                let path = entry.path();
                let is_manifest = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("appmanifest_") && name.ends_with(".acf"));
                if is_manifest {
                    candidates.push(path);
                }
            }
        }
        candidates.push(steam.join("steamapps/libraryfolders.vdf"));
        candidates.push(steam.join("config/libraryfolders.vdf"));
        candidates.push(steam.join("config/config.vdf"));
    }

    let mut tested = 0;
    for path in candidates {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let parsed = vdf::loads(&bytes)
            .unwrap_or_else(|err| panic!("parse failed for {}: {err}", path.display()));
        let dumped = vdf::dumps(&parsed);

        // Holds for every input: parsing loses nothing, and the output is a
        // fixed point.
        assert_eq!(
            vdf::loads(&dumped).unwrap(),
            parsed,
            "values lost in round-trip: {}",
            path.display()
        );
        assert_eq!(
            vdf::dumps(&vdf::loads(&dumped).unwrap()),
            dumped,
            "output is not stable: {}",
            path.display()
        );

        // Byte-identity is asserted for every file Steam writes in canonical
        // form. config.vdf is excluded on purpose: Valve puts raw newlines
        // inside quoted values there (SDL controller mappings), which come
        // back as `\n` escapes. steamtrain never writes that file.
        if name == "localconfig.vdf" || name == "libraryfolders.vdf" || name.ends_with(".acf") {
            assert_eq!(dumped, bytes, "round-trip mismatch: {}", path.display());
        }
        tested += 1;
    }
    if tested == 0 {
        eprintln!("no real Steam files on this machine; nothing round-tripped");
    } else {
        eprintln!("round-tripped {tested} real Steam file(s)");
    }
}
