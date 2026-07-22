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
        extra_vars: Mutex::new(HashMap::new()),
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
        c.extra_vars = Mutex::new(extra);
    });

    let result = cmd
        .bootstrap_cmd_id(&CmdType::Fetch, &ObjectId::from("obj1"))
        .await;
    assert_eq!(result, "fetch --id obj1 --token secret");
}

#[test]
fn bootstrap_cmd_object_substitutes_metadata_fields() {
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

    let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object).unwrap();
    assert_eq!(result, "put f1 file.txt 10 text/plain");
}

#[test]
fn bootstrap_cmd_object_defaults_missing_metadata_fields() {
    let cmd = make_command(|c| {
        c.put_cmd = "put {size} {content_type} {modified}".to_string();
    });
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object).unwrap();
    assert_eq!(result, "put 0 unknown ");
}

#[test]
fn bootstrap_cmd_object_uses_send_cmd_template_for_send() {
    let cmd = make_command(|c| {
        c.put_cmd = "put {id}".to_string();
        c.send_cmd = "send {id}".to_string();
    });
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };

    let result = cmd.bootstrap_cmd_object(&CmdType::Send, &object).unwrap();
    assert_eq!(result, "send f1");
}

#[test]
fn bootstrap_cmd_object_rejects_non_put_send_types() {
    let cmd = make_command(|_| {});
    let object = Object::Leaf {
        name: "file.txt".to_string(),
        id: ObjectId::from("f1"),
        meta: Metadata::new(),
    };
    let result = cmd.bootstrap_cmd_object(&CmdType::Get, &object);
    assert!(matches!(result, Err(VaultError::InvalidMethodCall)));
}

#[test]
fn bootstrap_cmd_object_errors_on_root_object() {
    let cmd = make_command(|_| {});
    let object = Object::root();
    let result = cmd.bootstrap_cmd_object(&CmdType::Put, &object);
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
