use super::*;
use crate::object::Metadata;

fn make_command(overrides: impl FnOnce(&mut OriginCommand)) -> OriginCommand {
    let mut cmd = OriginCommand {
        fetch_cmd: "echo fetch {id}".to_string(),
        list_cmd: "echo list {id}".to_string(),
        get_cmd: "echo get {id}".to_string(),
        put_cmd: "echo put {id} {name}".to_string(),
        send_cmd: "echo send {id}".to_string(),
        delete_cmd: "echo delete {id}".to_string(),
        rename_cmd: None,
        extra_vars: HashMap::new(),
    };
    overrides(&mut cmd);
    cmd
}

#[test]
fn cmd_type_as_str_maps_variants() {
    assert_eq!(CmdType::Fetch.as_str(), "fetch_cmd");
    assert_eq!(CmdType::List.as_str(), "list_cmd");
    assert_eq!(CmdType::Get.as_str(), "get_cmd");
    assert_eq!(CmdType::Put.as_str(), "put_cmd");
    assert_eq!(CmdType::Send.as_str(), "send_cmd");
    assert_eq!(CmdType::Delete.as_str(), "delete_cmd");
    assert_eq!(CmdType::Fetch.to_string(), "fetch_cmd");
}

#[test]
fn interpolate_one_replaces_placeholder() {
    let mut template = "hello {name}!".to_string();
    OriginCommand::interpolate_one(&mut template, "name", "world");
    assert_eq!(template, "hello world!");
}

#[test]
fn interpolate_replaces_all_vars() {
    let mut template = "{a} and {b}".to_string();
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), "1".to_string());
    vars.insert("b".to_string(), "2".to_string());
    OriginCommand::interpolate(&mut template, &vars);
    assert_eq!(template, "1 and 2");
}

#[tokio::test]
async fn bootstrap_cmd_id_substitutes_id_and_extra_vars() {
    let mut extra = HashMap::new();
    extra.insert("token".to_string(), "secret".to_string());
    let cmd = make_command(|c| {
        c.fetch_cmd = "fetch --id {id} --token {token}".to_string();
        c.extra_vars = extra;
    });

    let result = cmd
        .bootstrap_cmd_id(&CmdType::Fetch, &ObjectId::from("obj1"))
        .await;
    assert_eq!(result, "fetch --id obj1 --token secret");
}

#[tokio::test]
async fn bootstrap_cmd_object_substitutes_metadata_fields() {
    let cmd = make_command(|c| {
        c.put_cmd = "put {id} {name} {size} {content_type}".to_string();
    });
    let mut meta = Metadata::new();
    meta.set_size(10).set_content_type("text/plain".to_string());
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta,
    };

    let result = cmd
        .bootstrap_cmd_object(&CmdType::Put, &object)
        .await
        .unwrap();
    assert_eq!(result, "put f1 file.txt 10 text/plain");
}

#[tokio::test]
async fn bootstrap_cmd_object_defaults_missing_metadata_fields() {
    let cmd = make_command(|c| {
        c.put_cmd = "put {size} {content_type} {modified}".to_string();
    });
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let result = cmd
        .bootstrap_cmd_object(&CmdType::Put, &object)
        .await
        .unwrap();
    // An empty value is passed as an explicit empty argument (`''`) rather than vanishing,
    // which would shift every later argument left by one.
    assert_eq!(result, "put 0 unknown ''");
}

#[tokio::test]
async fn bootstrap_cmd_object_uses_send_cmd_template_for_send() {
    let cmd = make_command(|c| {
        c.put_cmd = "put {id}".to_string();
        c.send_cmd = "send {id}".to_string();
    });
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let result = cmd
        .bootstrap_cmd_object(&CmdType::Send, &object)
        .await
        .unwrap();
    assert_eq!(result, "send f1");
}

#[tokio::test]
async fn bootstrap_cmd_object_rejects_non_put_send_types() {
    let cmd = make_command(|_| {});
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let result = cmd.bootstrap_cmd_object(&CmdType::Get, &object).await;
    assert!(matches!(result, Err(VaultError::InvalidMethodCall)));
}

#[tokio::test]
async fn bootstrap_cmd_object_errors_on_root_object() {
    let cmd = make_command(|_| {});
    let object = Object::root();
    let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object).await;
    assert!(matches!(result, Err(VaultError::Unreachable)));
}

#[tokio::test]
async fn get_parses_json_output_from_command() {
    let cmd = make_command(|c| {
        c.get_cmd = r#"echo '{"Leaf":{"name":"file","id":"f1","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
    });

    let object = cmd.get(&ObjectId::from("f1")).await.unwrap();
    match object {
        Object::Leaf { name, id, .. } => {
            assert_eq!(name, "file");
            assert_eq!(id.as_str(), "f1");
        }
        _ => panic!("expected leaf"),
    }
}

#[tokio::test]
async fn list_parses_json_array_output() {
    let cmd = make_command(|c| {
        c.list_cmd = r#"echo '[{"Leaf":{"name":"a","id":"a","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}]'"#.to_string();
    });

    let objects = cmd.list(&ObjectId::from("dir")).await.unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].get_name(), "a");
}

#[tokio::test]
async fn cmd_returns_generic_error_on_nonzero_exit() {
    let cmd = make_command(|c| {
        c.get_cmd = "sh -c 'exit 1'".to_string();
    });
    let result = cmd.get(&ObjectId::from("f1")).await;
    assert!(matches!(result, Err(VaultError::Generic(_))));
}

#[tokio::test]
async fn put_succeeds_on_zero_exit() {
    let cmd = make_command(|c| {
        c.put_cmd = "true".to_string();
        c.get_cmd = r#"echo '{"Leaf":{"name":"file.txt","id":"{id}","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
    });
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    // put also runs a follow-up `get` on "{destination}/{name}" to return the created
    // Object; this must not deadlock on `extra_vars` (see
    // `put_does_not_deadlock_on_extra_vars_mutex` below).
    let result = cmd.put(&mut object, &ObjectId::from("dir")).await.unwrap();
    assert_eq!(result.get_id().as_str(), "dir/file.txt");
}

#[tokio::test]
async fn put_does_not_deadlock_on_extra_vars_mutex() {
    // `put` locks `extra_vars` to set the `destination` var, then (on success) calls
    // `self.get()` internally, which also locks `extra_vars`. If the first lock is still
    // held at that point, this hangs forever instead of erroring or returning.
    let cmd = make_command(|c| {
        c.put_cmd = "true".to_string();
        c.get_cmd = r#"echo '{"Leaf":{"name":"file.txt","id":"{id}","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
    });
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cmd.put(&mut object, &ObjectId::from("dir")),
    )
    .await;

    assert!(
        outcome.is_ok(),
        "put() deadlocked instead of completing within the timeout"
    );
    assert!(outcome.unwrap().is_ok());
}

#[tokio::test]
async fn put_errors_on_command_failure() {
    let cmd = make_command(|c| {
        c.put_cmd = "false".to_string();
    });
    let mut object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let result = cmd.put(&mut object, &ObjectId::from("")).await;
    assert!(matches!(result, Err(VaultError::Generic(_))));
}

#[tokio::test]
async fn delete_runs_configured_command() {
    let cmd = make_command(|c| {
        c.delete_cmd = "true".to_string();
    });
    cmd.delete(&ObjectId::from("f1")).await.unwrap();
}

#[tokio::test]
async fn fetch_streams_command_stdout() {
    let cmd = make_command(|c| {
        c.fetch_cmd = "printf hello".to_string();
    });
    let mut stream = cmd.fetch(&ObjectId::from("f1")).await.unwrap();
    let mut collected = Vec::new();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(collected, b"hello");
}

// --- regressions: templating available to put/send ---

#[tokio::test]
async fn bootstrap_cmd_object_substitutes_configured_extras() {
    // `put_cmd`/`send_cmd` used to interpolate only the *object's* metadata extras, so a config
    // referencing a shared `{helper}`/`{root}` ran with the literal braces still in it.
    let mut extras = HashMap::new();
    extras.insert("helper".to_string(), "/opt/helper.sh".to_string());
    extras.insert("root".to_string(), "/srv/data".to_string());
    let cmd = OriginCommand::new(
        "fetch".to_string(),
        "list".to_string(),
        "get".to_string(),
        "{helper} put {root} {name}".to_string(),
        "{helper} send {root} {id}".to_string(),
        "delete".to_string(),
        Some(extras),
    );
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let put = cmd
        .bootstrap_cmd_object(&CmdType::Put, &object)
        .await
        .unwrap();
    assert_eq!(put, "/opt/helper.sh put /srv/data file.txt");

    let send = cmd
        .bootstrap_cmd_object(&CmdType::Send, &object)
        .await
        .unwrap();
    assert_eq!(send, "/opt/helper.sh send /srv/data f1");
}

#[tokio::test]
async fn bootstrap_cmd_object_substitutes_the_object_kind() {
    // Without `{kind}` a `put_cmd` cannot tell a file from a directory, which makes `mkdir`
    // impossible to implement against a command origin.
    let cmd = make_command(|c| {
        c.put_cmd = "put {kind} {name}".to_string();
    });

    let leaf = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    assert_eq!(
        cmd.bootstrap_cmd_object(&CmdType::Put, &leaf)
            .await
            .unwrap(),
        "put leaf file.txt"
    );

    let branch = Object::Branch {
        name: "docs".to_string(),
        id: ObjectId::from("docs"),
        meta: Metadata::new(),
        children: None,
    };
    assert_eq!(
        cmd.bootstrap_cmd_object(&CmdType::Put, &branch)
            .await
            .unwrap(),
        "put branch docs"
    );
}

#[tokio::test]
async fn object_extras_take_precedence_over_configured_extras() {
    let mut extras = HashMap::new();
    extras.insert("target".to_string(), "default".to_string());
    let cmd = OriginCommand::new(
        "fetch".to_string(),
        "list".to_string(),
        "get".to_string(),
        "put {target}".to_string(),
        "send".to_string(),
        "delete".to_string(),
        Some(extras),
    );
    let mut meta = Metadata::new();
    meta.add_extra("target".to_string(), "per-object".to_string());
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta,
    };

    assert_eq!(
        cmd.bootstrap_cmd_object(&CmdType::Put, &object)
            .await
            .unwrap(),
        "put per-object"
    );
}

#[tokio::test]
async fn put_refreshes_the_destination_placeholder_on_every_call() {
    // `{destination}` used to be inserted only when absent, so every put after the first was
    // sent to wherever the first one happened to go.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();

    let cmd = OriginCommand::new(
        "fetch".to_string(),
        "list".to_string(),
        r#"printf '{"Leaf":{"name":"{name}","id":"{id}","meta":{"size":0,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string(),
        format!("touch {root}/{{destination}}/{{name}}"),
        "true".to_string(),
        "true".to_string(),
        None,
    );

    let mut first = Object::Leaf {
        name: "one.txt".to_string(),
        id: ObjectId::from("one.txt"),
        meta: Metadata::new(),
    };
    cmd.put(&mut first, &ObjectId::from("a")).await.unwrap();

    let mut second = Object::Leaf {
        name: "two.txt".to_string(),
        id: ObjectId::from("two.txt"),
        meta: Metadata::new(),
    };
    cmd.put(&mut second, &ObjectId::from("b")).await.unwrap();

    assert!(dir.path().join("a/one.txt").exists());
    assert!(
        dir.path().join("b/two.txt").exists(),
        "second put went to the first put's destination"
    );
}

// --- regressions: placeholders are shell-quoted ---

#[test]
fn sh_quote_leaves_plain_words_alone() {
    for plain in [
        "file.txt",
        "/srv/data",
        "dir/a-b_c",
        "text/plain",
        "user@host:22",
    ] {
        assert_eq!(sh_quote(plain), plain);
    }
}

#[test]
fn sh_quote_wraps_everything_else_in_single_quotes() {
    assert_eq!(sh_quote(""), "''");
    assert_eq!(sh_quote("sp ace"), "'sp ace'");
    assert_eq!(sh_quote("a=b"), "'a=b'");
    assert_eq!(sh_quote("$(id)"), "'$(id)'");
    assert_eq!(sh_quote("it's"), r"'it'\''s'");
}

/// Expands `printf %s {name}` the way `put` would, runs it, and returns what was printed —
/// i.e. the single word the shell actually received for `{name}`.
async fn word_seen_by_shell(name: &str) -> String {
    let cmd = make_command(|c| c.put_cmd = "printf %s {name}".to_string());
    let object = Object::Leaf {
        name: name.to_string(),
        id: ObjectId::from("x"),
        meta: Metadata::new(),
    };
    let line = cmd
        .bootstrap_cmd_object(&CmdType::Put, &object)
        .await
        .unwrap();
    let output = Command::new("sh")
        .arg("-c")
        .arg(&line)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{line}");
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn hostile_names_reach_the_command_as_one_literal_word() {
    for name in [
        "x'; touch INJECTED; echo '",
        "$(touch INJECTED)",
        "`touch INJECTED`",
        "a b  c",
        "semi;colon && amp | pipe > redirect",
        "it's \"quoted\" \\ back",
        "*",
    ] {
        assert_eq!(word_seen_by_shell(name).await, name);
    }
}

#[tokio::test]
async fn injected_command_in_a_name_does_not_run() {
    // The reproduction from the scan: this name used to run `touch INJECTED`.
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("INJECTED");
    let name = format!("x'; touch {}; echo '", marker.display());
    let cmd = make_command(|c| {
        c.put_cmd = "echo creating {name} >/dev/null".to_string();
        c.get_cmd = r#"echo '{"Leaf":{"name":"x","id":"x","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
    });
    let mut object = Object::Leaf {
        name,
        id: ObjectId::from("x"),
        meta: Metadata::new(),
    };
    let _ = cmd.put(&mut object, &ObjectId::from("")).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn bootstrap_cmd_id_quotes_ids_and_extras() {
    let mut extra = HashMap::new();
    extra.insert("root".to_string(), "/my data".to_string());
    let cmd = make_command(|c| {
        c.fetch_cmd = "cat {root}/data/{id}".to_string();
        c.extra_vars = extra;
    });
    let line = cmd
        .bootstrap_cmd_id(&CmdType::Fetch, &ObjectId::from("a#b c.txt"))
        .await;
    // Adjacent quoted pieces join into one word: `/my data/data/a#b c.txt`.
    assert_eq!(line, "cat '/my data'/data/'a#b c.txt'");
}

#[tokio::test]
async fn put_rejects_names_that_are_not_one_path_component() {
    let cmd = make_command(|c| c.put_cmd = "true".to_string());
    for name in ["../x", "a/b", ".."] {
        let mut object = Object::Leaf {
            name: name.to_string(),
            id: ObjectId::from("x"),
            meta: Metadata::new(),
        };
        let result = cmd.put(&mut object, &ObjectId::from("")).await;
        assert!(
            matches!(result, Err(VaultError::InvalidName(_))),
            "{name:?}"
        );
    }
}

// --- regressions: fetch reports a failing fetch_cmd ---

async fn drain(stream: ByteStream) -> (Vec<u8>, Option<VaultError>) {
    let mut bytes = Vec::new();
    let mut stream = stream;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(e) => return (bytes, Some(e)),
        }
    }
    (bytes, None)
}

#[tokio::test]
async fn fetch_ends_with_an_error_when_the_command_fails() {
    // Used to end "cleanly" after `half`, which was then saved as the complete object.
    let cmd = make_command(|c| {
        c.fetch_cmd = "printf half; echo 'connection reset' >&2; exit 3".to_string();
    });
    let (bytes, error) = drain(cmd.fetch(&ObjectId::from("doc.txt")).await.unwrap()).await;
    assert_eq!(bytes, b"half");
    let error = error.expect("a failing fetch_cmd must end the stream with an error");
    let message = error.to_string();
    assert!(message.contains("connection reset"), "{message}");
    assert!(message.contains("doc.txt"), "{message}");
}

#[tokio::test]
async fn fetch_with_a_successful_command_has_no_trailing_error() {
    let cmd = make_command(|c| c.fetch_cmd = "printf hello; echo noise >&2".to_string());
    let (bytes, error) = drain(cmd.fetch(&ObjectId::from("x")).await.unwrap()).await;
    assert_eq!(bytes, b"hello");
    assert!(error.is_none(), "{error:?}");
}

#[tokio::test]
async fn fetch_survives_a_command_that_floods_stderr() {
    // stderr is drained concurrently; otherwise a full pipe would deadlock the child.
    let cmd = make_command(|c| {
        c.fetch_cmd = "head -c 1000000 /dev/zero >&2; printf done".to_string();
    });
    let fetched = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        drain(cmd.fetch(&ObjectId::from("x")).await.unwrap()).await
    })
    .await
    .expect("fetch deadlocked on a full stderr pipe");
    assert_eq!(fetched.0, b"done");
    assert!(fetched.1.is_none());
}

// --- regressions: a failed payload doesn't let send_cmd commit ---

#[tokio::test]
async fn send_kills_the_command_when_the_payload_fails() {
    use futures::stream;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::write(&out, b"original").unwrap();
    // Commits whatever arrived once stdin closes: what a naive send_cmd does.
    let cmd = make_command(|c| {
        c.send_cmd = format!("cat > {0}.part && mv {0}.part {0}", out.display());
    });
    let object = Object::Leaf {
        name: "out".to_string(),
        id: ObjectId::from("out"),
        meta: Metadata::new(),
    };
    let payload: ByteStream = Box::pin(stream::iter(vec![
        Ok(bytes::Bytes::from_static(b"partial ")),
        Err(VaultError::OriginError("connection dropped".to_string())),
    ]));

    let result = cmd.send(&object, payload).await;
    assert!(
        matches!(result, Err(VaultError::OriginError(_))),
        "{result:?}"
    );
    // Give a surviving child time to (wrongly) finish its `mv`.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(std::fs::read(&out).unwrap(), b"original");
}

#[tokio::test]
async fn send_still_commits_a_complete_payload() {
    use futures::stream;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let cmd = make_command(|c| {
        c.send_cmd = format!("cat > {0}.part && mv {0}.part {0}", out.display());
    });
    let object = Object::Leaf {
        name: "out".to_string(),
        id: ObjectId::from("out"),
        meta: Metadata::new(),
    };
    let payload: ByteStream = Box::pin(stream::iter(vec![
        Ok(bytes::Bytes::from_static(b"hello ")),
        Ok(bytes::Bytes::from_static(b"world")),
    ]));
    cmd.send(&object, payload).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"hello world");
}

// --- rename ---

#[tokio::test]
async fn rename_without_a_rename_cmd_is_unsupported() {
    let cmd = make_command(|_| {});
    let result = cmd
        .rename(&ObjectId::from("a"), &ObjectId::from(""), "b")
        .await;
    assert!(matches!(result, Err(VaultError::Unsupported(_))));
}

#[tokio::test]
async fn rename_runs_rename_cmd_with_quoted_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("dst dir")).unwrap();
    std::fs::write(dir.path().join("a b.txt"), b"x").unwrap();
    let mut extras = HashMap::new();
    extras.insert(
        "root".to_string(),
        dir.path().to_string_lossy().into_owned(),
    );
    let cmd = make_command(|c| {
        c.extra_vars = extras;
        c.get_cmd = r#"echo '{"Leaf":{"name":"n","id":"i","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
    })
    .with_rename_cmd(Some("mv {root}/{id} {root}/{destination}/{name}".to_string()));

    cmd.rename(
        &ObjectId::from("a b.txt"),
        &ObjectId::from("dst dir"),
        "c d.txt",
    )
    .await
    .unwrap();
    assert!(!dir.path().join("a b.txt").exists());
    assert_eq!(
        std::fs::read(dir.path().join("dst dir/c d.txt")).unwrap(),
        b"x"
    );
}

// --- regressions: one-pass rendering; per-call destination ---

#[test]
fn render_never_rescans_substituted_text() {
    // Replacing placeholders one at a time re-scanned inserted text: a name of `{x}` plus an
    // extra `x = "$(cmd)"` became `''$(cmd)''`, i.e. an unquoted command substitution.
    let vars = HashMap::from([
        ("name".to_string(), "{x}".to_string()),
        ("x".to_string(), "$(touch INJECTED)".to_string()),
    ]);
    assert_eq!(render("echo {name}", &vars), "echo '{x}'");
}

#[test]
fn render_leaves_unknown_keys_and_stray_braces_alone() {
    let vars = HashMap::from([("id".to_string(), "a.txt".to_string())]);
    assert_eq!(
        render(r#"echo '{"id":"{id}","x":"{other}"}' ${HOME}"#, &vars),
        r#"echo '{"id":"a.txt","x":"{other}"}' ${HOME}"#
    );
    assert_eq!(render("{ {id} }", &vars), "{ a.txt }");
    assert_eq!(render("trailing {", &vars), "trailing {");
}

#[tokio::test]
async fn object_extras_cannot_inject_through_a_crafted_name() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("INJECTED");
    let cmd = make_command(|c| c.send_cmd = "true {name}".to_string());
    let mut meta = Metadata::new();
    meta.add_extra("x".to_string(), format!("$(touch {})", marker.display()));
    let object = Object::Leaf {
        name: "{x}".to_string(),
        id: ObjectId::from("x"),
        meta,
    };
    let line = cmd
        .bootstrap_cmd_object(&CmdType::Send, &object)
        .await
        .unwrap();
    let status = Command::new("sh")
        .arg("-c")
        .arg(&line)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "{line}");
    assert!(!marker.exists(), "{line}");
}

#[tokio::test]
async fn concurrent_puts_each_use_their_own_destination() {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["one", "two"] {
        std::fs::create_dir(dir.path().join(sub)).unwrap();
    }
    let root = dir.path().to_string_lossy().into_owned();
    // A slow put_cmd widens the window in which shared state would be overwritten.
    let cmd = std::sync::Arc::new(make_command(|c| {
        c.put_cmd = format!("sleep 0.2; touch {root}/{{destination}}/{{name}}");
        c.get_cmd = r#"echo '{"Leaf":{"name":"n","id":"i","meta":{"size":null,"content_type":null,"modified":null,"extra":{}}}}'"#.to_string();
    }));
    let put = |dest: &'static str, name: &'static str| {
        let cmd = cmd.clone();
        async move {
            let mut object = Object::Leaf {
                name: name.to_string(),
                id: ObjectId::from(name),
                meta: Metadata::new(),
            };
            cmd.put(&mut object, &ObjectId::from(dest)).await.unwrap();
        }
    };
    tokio::join!(put("one", "a.txt"), put("two", "b.txt"));
    assert!(dir.path().join("one/a.txt").exists());
    assert!(dir.path().join("two/b.txt").exists());
}

#[tokio::test]
async fn put_into_root_slash_does_not_ask_for_a_double_slash_id() {
    // The follow-up `get` used to be run for `//a.txt`.
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let cmd = make_command(|c| {
        c.put_cmd = "true".to_string();
        c.get_cmd = format!(
            r#"printf %s {{id}} > {}; echo '{{"Leaf":{{"name":"a.txt","id":"a.txt","meta":{{"size":null,"content_type":null,"modified":null,"extra":{{}}}}}}}}'"#,
            seen.display()
        );
    });
    let mut object = Object::Leaf {
        name: "a.txt".to_string(),
        id: ObjectId::from("a.txt"),
        meta: Metadata::new(),
    };
    cmd.put(&mut object, &ObjectId::from("/")).await.unwrap();
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "/a.txt");
}
