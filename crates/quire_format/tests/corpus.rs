//! NR22's parity gate: the NR06 corpus (21 snippets, one or more per fence tag rt formats) must
//! format byte for byte as rt's Prettier 3.9.10 call did when the expected files were recorded
//! by `spikes/nr06/format/prettier-ref.ts`.

use quire_format::{Formatter, Parser};
use std::{fs, path::Path};

#[test]
fn corpus_matches_recorded_prettier_output_byte_for_byte() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut inputs: Vec<_> = fs::read_dir(root.join("input"))
        .expect("tests/corpus/input exists")
        .map(|entry| entry.unwrap().path())
        .collect();
    inputs.sort();
    assert_eq!(inputs.len(), 21, "the NR06 corpus has 21 snippets");
    let formatter = Formatter::default();
    let mismatches: Vec<_> = inputs
        .iter()
        .filter_map(|input| {
            let name = input.file_name().unwrap().to_string_lossy().into_owned();
            let tag = input.extension().unwrap().to_string_lossy();
            let parser = Parser::for_tag(&tag).unwrap_or_else(|| panic!("{name} has no parser"));
            let text = fs::read_to_string(input).unwrap();
            let expected = fs::read_to_string(root.join("expected").join(&name)).unwrap();
            let actual = futures::executor::block_on(formatter.format(text, parser))
                .unwrap_or_else(|error| format!("ERROR: {error}\n"));
            (actual != expected)
                .then(|| format!("{name}:\n--- expected\n{expected}--- actual\n{actual}"))
        })
        .collect();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
