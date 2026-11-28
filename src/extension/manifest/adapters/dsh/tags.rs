use std::mem::MaybeUninit;

// yaml_serde retains local tags but interprets unknown !! tags as scalars.
// Preserve explicit tags as local markers so the literal importer can reject
// exactly the contribution containing any tagged node.
pub(super) fn preserve_explicit_tags(source: &str) -> Result<String, String> {
    use libyaml_rs::{
        YAML_NO_TOKEN, YAML_STREAM_END_TOKEN, YAML_TAG_DIRECTIVE_TOKEN, YAML_TAG_TOKEN,
        yaml_parser_delete, yaml_parser_initialize, yaml_parser_scan, yaml_parser_set_input_string,
        yaml_parser_t, yaml_token_delete, yaml_token_t,
    };

    struct Parser(yaml_parser_t);
    impl Drop for Parser {
        fn drop(&mut self) {
            // SAFETY: initialization succeeded and only this guard owns the parser.
            unsafe { yaml_parser_delete(&mut self.0) };
        }
    }

    let mut storage = MaybeUninit::<yaml_parser_t>::uninit();
    // SAFETY: the initializer takes a writable parser storage slot.
    let mut parser = unsafe {
        if !yaml_parser_initialize(storage.as_mut_ptr()).ok {
            return Err("could not initialize the YAML parser".into());
        }
        Parser(storage.assume_init())
    };
    // SAFETY: the input slice remains allocated and unmodified until parser drop.
    unsafe { yaml_parser_set_input_string(&mut parser.0, source.as_ptr(), source.len() as u64) };
    let mut tags = Vec::new();
    loop {
        let mut storage = MaybeUninit::<yaml_token_t>::uninit();
        // SAFETY: parser is initialized; a successful scan initializes the token.
        let (kind, start, end) = unsafe {
            if !yaml_parser_scan(&mut parser.0, storage.as_mut_ptr()).ok {
                let mark = parser.0.problem_mark;
                return Err(format!(
                    "invalid YAML at line {}, column {}",
                    mark.line + 1,
                    mark.column + 1
                ));
            }
            let mut token = storage.assume_init();
            let value = (token.type_, token.start_mark, token.end_mark);
            yaml_token_delete(&mut token);
            value
        };
        match kind {
            YAML_STREAM_END_TOKEN => break,
            YAML_TAG_TOKEN => tags.push((start.index as usize, end.index as usize)),
            YAML_TAG_DIRECTIVE_TOKEN => {
                return Err(format!(
                    "unsupported tag directive at line {}",
                    start.line + 1
                ));
            }
            YAML_NO_TOKEN => return Err("YAML scanner returned no token".into()),
            _ => {}
        }
    }
    if tags.is_empty() {
        return Ok(source.to_owned());
    }
    let mut rewritten = String::with_capacity(source.len());
    let mut offset = 0;
    for (start, end) in tags {
        let spelling = source
            .get(start..end)
            .filter(|spelling| spelling.starts_with('!'))
            .ok_or_else(|| {
                format!(
                    "invalid YAML tag span {start}..{end}: {:?}",
                    source.get(start..end)
                )
            })?;
        rewritten.push_str(source.get(offset..start).ok_or("overlapping YAML tags")?);
        rewritten.push_str("!aleph-dsh-explicit-tag-");
        for byte in spelling.as_bytes() {
            use std::fmt::Write;
            let _ = write!(rewritten, "{byte:02x}");
        }
        offset = end;
    }
    rewritten.push_str(&source[offset..]);
    Ok(rewritten)
}

#[cfg(test)]
#[test]
fn distinguishes_tagged_values_and_plain_strings() {
    for source in [
        "标记: literal\nconfig: {command: !!js /test, args: ['!!js example']}\n",
        "# !!js comment\nconfig: {command: !!js /test, args: ['!!js example']}\n",
    ] {
        let output = preserve_explicit_tags(source).unwrap();
        let literal: yaml_serde::Value = yaml_serde::from_str(&output).unwrap();
        assert!(matches!(
            literal["config"]["command"],
            yaml_serde::Value::Tagged(_)
        ));
        assert_eq!(literal["config"]["args"][0].as_str(), Some("!!js example"));
    }
}
