// A throwaway peer for the real Claude CLI. The fixture sends scenario steps and
// receives the unfiltered CLI frames, including callbacks the SDK would consume.
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    fs::File,
    io::{self, BufRead, BufReader, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn line(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(writer, "{value}")?;
    writer.flush()
}

fn main() -> Result<(), Box<dyn Error>> {
    let scratch = env::var("SCRATCH")?;
    let mut trace = File::create(env::var("TRACE_WIRE")?)?;
    let mut args = env::args().skip(1);
    let executable = args.next().ok_or("missing CLI path")?;
    // The wrapper supplies this marker and confines both the model URL and CLI.
    if !executable.starts_with(&format!("{scratch}/bin/")) {
        return Err("expected the scratch copy of Claude Code".into());
    }
    let session = args.next().ok_or("missing session ID")?;
    let mode = args.next().ok_or("missing session mode")?;
    let settings = args.next().ok_or("missing settings JSON")?;
    let mut command = Command::new(executable);
    command.args([
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
    ]);
    if mode == "generate" {
        command.args([
            "--tools=",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--max-turns",
            "1",
            "--model",
            "haiku",
            "--settings",
            &settings,
        ]);
    } else {
        command.args([
            "--permission-prompt-tool=stdio",
            "--model",
            "sonnet",
            "--effort",
            "low",
            "--permission-mode",
            "default",
            "--include-partial-messages",
            "--plugin-dir-no-mcp",
            &format!("{scratch}/plugin"),
            "--settings",
            &settings,
            if mode == "resume" {
                "--resume"
            } else {
                "--session-id"
            },
            &session,
        ]);
    }
    command
        .env("CLAUDE_CODE_ENTRYPOINT", "sdk-ts")
        .env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let reader = thread::spawn(move || -> io::Result<()> {
        let mut out = io::stdout().lock();
        for frame in BufReader::new(stdout).lines() {
            writeln!(out, "{}", frame?)?;
            out.flush()?;
        }
        Ok(())
    });

    let outcome = (|| -> Result<(), Box<dyn Error>> {
        for input in io::stdin().lock().lines() {
            let step: Value = serde_json::from_str(&input?)?;
            let frame = match step["kind"].as_str() {
                Some("initialize") => {
                    let mut request = json!({"subtype":"initialize"});
                    if mode == "generate" {
                        request["systemPrompt"] = json!([""]);
                    } else {
                        request["appendSystemPrompt"] = json!("SPIKE_APPENDED_INSTRUCTIONS");
                        request["agents"] = json!({"spike-child":{
                            "description":"A local fixture subagent",
                            "prompt":"SPIKE_CHILD_INSTRUCTIONS",
                            "model":"haiku"
                        }});
                        request["forwardSubagentText"] = json!(true);
                        request["hooks"] = json!({"PreToolUse":[{"hookCallbackIds":["0"]}]});
                    }
                    json!({"type":"control_request","request_id":step["id"],"request":request})
                }
                Some("control") => json!({
                    "type":"control_request","request_id":step["id"],"request":step["request"]
                }),
                Some("callback") => json!({
                    "type":"control_response",
                    "response":{
                        "subtype":"success","request_id":step["id"],"response":step["response"]
                    }
                }),
                Some("message") => step["message"].clone(),
                _ => return Err("unknown scenario step".into()),
            };
            line(&mut trace, &frame)?;
            line(&mut stdin, &frame)?;
        }
        Ok(())
    })();
    drop(stdin);
    if outcome.is_err() {
        let _ = child.kill();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    reader.join().expect("CLI reader panicked")?;
    outcome?;
    if !status.success() {
        return Err(format!("Claude CLI exited {status}").into());
    }
    Ok(())
}
