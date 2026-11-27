use super::*;

fn bundle(patch: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"name":"qa-dsh","version":"1.0.0","dsh":{"bundle":{"patch":"./cordis.patch.yml"}}}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("cordis.patch.yml"), patch).unwrap();
    dir
}

const LITERAL: &str = r#"
- insert:
    - id: stdio-demo
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: stdio-demo
        transport: stdio
        command: python3
        args: [server.py, 'a b']
        cwd: ./work
        env: {SAMPLE: '42'}
    - id: http-demo
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: http-demo
        transport: streamable-http
        url: 'http://127.0.0.1:19731/mcp'
        headers: {X-Sample: enabled}
"#;

#[test]
fn dsh_literal_stdio_and_http_map_to_existing_mcp_configs() {
    let dir = bundle(LITERAL);
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let skill = dir.path().join("skills/example");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: example\ndescription: Example skill\n---\nExample body\n",
    )
    .unwrap();

    let output = DshAdapter.parse(dir.path()).unwrap();
    assert_eq!(output.source.format, SourceFormat::DeepSeekHarness);
    assert!(output.diagnostics.is_empty());
    assert_eq!(output.capabilities.len(), 3);
    assert!(output.capabilities.iter().any(
        |decl| matches!(decl, CapabilityDeclaration::Skill(skill) if skill.name == "example")
    ));
    let servers: Vec<_> = output
        .capabilities
        .iter()
        .filter_map(|decl| match decl {
            CapabilityDeclaration::McpServer(server) => Some(server),
            _ => None,
        })
        .collect();
    assert_eq!(servers[0].id, "plugin:qa-dsh/stdio-demo");
    assert_eq!(servers[0].command.as_deref(), Some("python3"));
    assert_eq!(servers[0].args, ["server.py", "a b"]);
    assert_eq!(
        servers[0].cwd.as_deref(),
        Some(work.canonicalize().unwrap().as_path())
    );
    assert_eq!(servers[0].env.get("SAMPLE").map(String::as_str), Some("42"));
    assert_eq!(servers[1].id, "plugin:qa-dsh/http-demo");
    assert_eq!(
        servers[1].url.as_deref(),
        Some("http://127.0.0.1:19731/mcp")
    );
    assert_eq!(
        servers[1].headers.get("X-Sample").map(String::as_str),
        Some("enabled")
    );
}

#[test]
fn dsh_keeps_literal_sibling_and_diagnoses_unmapped_node() {
    let dir = bundle(&format!(
        "{LITERAL}\n{}",
        r#"
- insert:
    - id: startup-policy
      name: '@deepseek-ai/dsh-mcp-client'
      config: {serverName: startup, transport: stdio, command: python3, failOnStartupError: true}
    - id: js-expression
      name: '@deepseek-ai/dsh-mcp-client'
      config: {serverName: expression, transport: stdio, command: !!js 'writeMarker()'}
    - id: required-service
      name: '@deepseek-ai/dsh-mcp-client'
      inject: [database]
      config: {serverName: service, transport: stdio, command: python3}
    - id: unsupported-field
      name: '@deepseek-ai/dsh-mcp-client'
      config: {serverName: field, transport: streamable-http, url: 'http://localhost/mcp', command: python3}
- replace: []
"#
    ));
    std::fs::create_dir(dir.path().join("work")).unwrap();
    let output = DshAdapter.parse(dir.path()).unwrap();
    assert_eq!(output.capabilities.len(), 2);
    let imports = output.import_diagnostics();
    assert!(!imports.is_empty());
    assert!(
        imports
            .iter()
            .all(|item| item.path.as_deref() == Some("cordis.patch.yml"))
    );
    assert_eq!(output.diagnostics.len(), 5);
    for (index, token) in [
        "failOnStartupError",
        "!!js",
        "required-service",
        "'command'",
        "literal insert",
    ]
    .into_iter()
    .enumerate()
    {
        let item = &output.diagnostics[index];
        assert!(item.path.ends_with("cordis.patch.yml"));
        assert!(item.message.contains("patch["), "{}", item.message);
        assert!(item.message.contains(token), "{}", item.message);
    }
}

#[test]
fn dsh_bad_patch_escape_or_duplicate_identity_refuses_package() {
    for (patch, expected) in [
        ("- insert: [", "invalid DSH patch"),
        ("- replace: []", "no mountable contributions"),
    ] {
        let dir = bundle(patch);
        let error = format!("{:#}", DshAdapter.parse(dir.path()).unwrap_err());
        assert!(error.contains(expected), "expected {expected:?}: {error}");
    }
    let dir = bundle(&LITERAL.repeat(2));
    std::fs::create_dir(dir.path().join("work")).unwrap();
    assert!(
        DshAdapter
            .parse(dir.path())
            .unwrap_err()
            .to_string()
            .contains("duplicate DSH serverName")
    );
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"dsh":{"bundle":{"patch":"../other.yml"}}}"#,
    )
    .unwrap();
    assert!(DshAdapter.parse(dir.path()).is_err());
}
