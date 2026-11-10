use kou_domain::{Input, ItemData, Json, Session, Timestamp, Turn};
use kou_store::SessionStore;
use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() < 2 {
        return Err("usage: fixture create DIR | append DIR ID | hold DIR ID".into());
    }
    let directory = PathBuf::from(&args[1]);
    let now = Timestamp::parse("2026-09-01T12:00:00Z")?;
    match args[0].as_str() {
        "create" if args.len() == 2 => {
            fs::create_dir_all(&directory)?;
            if fs::read_dir(&directory)?.next().is_some() {
                return Err("fixture destination must be empty".into());
            }
            let store = SessionStore::new(directory)?;
            let mut writer = store.create_new(Session {
                id: "rust-session".into(),
                created_at: now,
            })?;
            writer.append_item(
                ItemData::Input(Input {
                    id: "rust-input".into(),
                    kind: "external".into(),
                    payload: Json::parse(r#"{"message":"from Rust"}"#)?,
                    delivery: String::new(),
                }),
                now,
            )?;
            writer.append_item(
                ItemData::Turn(Turn {
                    id: "rust-turn".into(),
                    kind: "regular".into(),
                    model: "test".into(),
                    ..Turn::default()
                }),
                now,
            )?;
        }
        "large-checkpoint" if args.len() == 3 => {
            let bytes: usize = args[2].parse()?;
            if bytes == 0 || bytes > 128 * 1024 * 1024 {
                return Err("synthetic body must be 1..128 MiB".into());
            }
            let store = SessionStore::new(directory)?;
            let mut writer = store.create_new(Session {
                id: "rust-session".into(),
                created_at: now,
            })?;
            writer.append_item(
                ItemData::Input(Input {
                    id: "synthetic-input".into(),
                    kind: "external".into(),
                    payload: Json::parse(&serde_json::to_string(&"x".repeat(bytes))?)?,
                    delivery: String::new(),
                }),
                now,
            )?;
            writer.append_item(
                ItemData::Turn(Turn {
                    id: "synthetic-turn".into(),
                    kind: "regular".into(),
                    ..Turn::default()
                }),
                now,
            )?;
            writer.save_checkpoint(Json::parse(r#"{"generation":1}"#)?)?;
        }
        "append" if args.len() == 3 => {
            let store = SessionStore::new(directory)?;
            let mut writer = store.open_writer(&args[2])?;
            writer.append_item(
                ItemData::Input(Input {
                    id: "rust-continued".into(),
                    kind: "external".into(),
                    payload: Json::parse(r#"{"message":"continued by Rust"}"#)?,
                    delivery: String::new(),
                }),
                now,
            )?;
        }
        "fork" if args.len() == 5 => {
            let store = SessionStore::new(directory)?;
            let _writer = store.fork_new(
                Session {
                    id: args[3].clone(),
                    created_at: now,
                },
                &args[2],
                &args[4],
            )?;
        }
        "checkpoint" if args.len() == 3 => {
            let store = SessionStore::new(directory)?;
            store
                .open_writer(&args[2])?
                .save_checkpoint(Json::parse(r#"{"generation":1}"#)?)?;
        }
        "hold" if args.len() == 3 => {
            let store = SessionStore::new(directory)?;
            let _writer = store.open_writer(&args[2])?;
            println!("locked");
            io::stdin().read_to_end(&mut Vec::new())?;
        }
        _ => return Err("incorrect fixture arguments".into()),
    }
    Ok(())
}
