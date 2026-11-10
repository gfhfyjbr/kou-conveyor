use kou_domain::Snapshot;
use kou_store::SessionLog;
use serde::{
    Serialize,
    ser::{SerializeSeq, SerializeStruct},
};
use std::{
    env,
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    os::unix::fs::OpenOptionsExt,
    process::ExitCode,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kou-session: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() < 2 {
        return Err("usage: kou-session inspect|archive|items|resume|resume-full|copy FILE [AFTER LIMIT | DESTINATION]".into());
    }
    let command = args[0].to_str().ok_or("non-UTF-8 command")?;
    let expected = match command {
        "items" => 4,
        "copy" => 3,
        "inspect" | "archive" | "resume-full" | "resume" => 2,
        _ => return Err("unknown command".into()),
    };
    if args.len() != expected {
        return Err("incorrect number of arguments".into());
    }
    if command == "resume" {
        let state = kou_store::SessionRecovery::open(&args[1])?.into_resume()?;
        serde_json::to_writer(io::stdout().lock(), &state)?;
        return Ok(());
    }
    let log = SessionLog::open(&args[1])?;
    if command == "copy" {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&args[2])?;
        log.copy_committed(&mut out)?;
        out.sync_all()?;
        return Ok(());
    }
    let mut out = BufWriter::new(io::stdout().lock());
    match command {
        "inspect" => serde_json::to_writer(&mut out, &log.summary())?,
        "resume-full" => serde_json::to_writer(&mut out, &log.resume_full()?)?,
        "items" => {
            let after: u64 = args[2].to_str().ok_or("invalid cursor")?.parse()?;
            let limit: usize = args[3].to_str().ok_or("invalid limit")?.parse()?;
            serde_json::to_writer(&mut out, &log.items(after, limit)?)?;
        }
        "archive" => serde_json::to_writer(&mut out, &Archive(&log))?,
        _ => unreachable!(),
    }
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

struct Archive<'a>(&'a SessionLog);

impl Serialize for Archive<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let resume = self.0.resume_full().map_err(serde::ser::Error::custom)?;
        let mut out = serializer.serialize_struct("Archive", 4)?;
        out.serialize_field(
            "Snapshot",
            &Snapshot {
                session: self.0.session().clone(),
            },
        )?;
        out.serialize_field("Items", &Items(self.0))?;
        out.serialize_field("Operations", &resume.operations)?;
        out.serialize_field("ExternalInputIDs", &resume.external_input_ids)?;
        out.end()
    }
}

struct Items<'a>(&'a SessionLog);

impl Serialize for Items<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_seq(Some(self.0.item_count()))?;
        for sequence in 1..=self.0.item_count() as u64 {
            let item = self.0.item(sequence).map_err(serde::ser::Error::custom)?;
            out.serialize_element(&item)?;
        }
        out.end()
    }
}
