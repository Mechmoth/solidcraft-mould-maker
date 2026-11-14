//! Run the plugin's logic natively against `solidcraft-cli mcp --headless`, timing every call.
//! Same command sequence as the Wasm build; for profiling and debugging, not shipped.
//!   cargo run --release --example host_run -- <solidcraft-cli> <design.sc> '<params json>' [save.sc]
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use mould_maker::{parse_params, run_entry, Runner};
use serde_json::{json, Value};

struct Mcp {
    _child: Child,
    tx: ChildStdin,
    rx: BufReader<ChildStdout>,
    id: u64,
    log: Vec<(String, f64)>,
}

impl Mcp {
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        writeln!(self.tx, "{msg}").unwrap();
        self.tx.flush().unwrap();
        loop {
            let mut line = String::new();
            if self.rx.read_line(&mut line).unwrap() == 0 {
                panic!("host exited");
            }
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(self.id) {
                return v;
            }
        }
    }
    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        let text = res["content"][0]["text"].as_str().unwrap_or("").to_string();
        if res["isError"].as_bool() == Some(true) || r.get("error").is_some() {
            return Err(format!("{text}{}", r.get("error").map(|e| e.to_string()).unwrap_or_default()));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

impl Runner for Mcp {
    fn run(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let t = Instant::now();
        let r = self.tool("execute", json!({"command": command, "params": args.clone()}));
        if let Err(e) = &r {
            let a = args.to_string();
            eprintln!("FAILED CALL #{}: {command} {}\n  -> {e}", self.log.len(), &a[..a.len().min(1500)]);
        }
        self.log.push((command.to_string(), t.elapsed().as_secs_f64() * 1000.0));
        r
    }
    fn log(&mut self, m: &str) {
        eprintln!("[log] {m}");
    }
    fn progress(&mut self, f: f32, m: &str) {
        let total: f64 = self.log.iter().map(|x| x.1).sum();
        eprintln!("[{:>5.1}s] {:>3.0}% {m} ({} calls)", total / 1000.0, f * 100.0, self.log.len());
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[2]).parent().unwrap().to_str().unwrap().to_string();
    let mut child = Command::new(&a[1])
        .args(["mcp", "--headless", "--in", &a[2], "--allow-path", &dir])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let tx = child.stdin.take().unwrap();
    let rx = BufReader::new(child.stdout.take().unwrap());
    let mut h = Mcp { _child: child, tx, rx, id: 0, log: vec![] };
    h.rpc("initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "host_run", "version": "1"}}));
    writeln!(h.tx, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
    let pv: Value = serde_json::from_str(&a[3]).unwrap();
    let runs: Vec<Value> = if let Value::Array(v) = pv { v } else { vec![pv] };
    let t = Instant::now();
    let mut r = Err("no runs".to_string());
    for (i, one) in runs.iter().enumerate() {
        if i > 0 {
            let names = h.tool("measure", json!({"names_only": true})).unwrap();
            eprintln!("bodies before run {i}: {}", names["bodies"].as_array().map(|a| a.iter().map(|b| b["name"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default());
        }
        if let Some(sel) = one.get("select") {
            h.tool("execute", json!({"command": "select.set", "params": {"items": sel}})).unwrap();
        }
        let entry = one.get("entry").and_then(Value::as_str).unwrap_or("round-mould").to_string();
        r = run_entry(&mut h, &entry, &parse_params(one));
        if r.is_err() {
            break;
        }
    }
    let wall = t.elapsed().as_secs_f64();
    let mut by: std::collections::BTreeMap<String, (usize, f64)> = Default::default();
    for (c, ms) in &h.log {
        let e = by.entry(c.clone()).or_default();
        e.0 += 1;
        e.1 += ms;
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by(|x, y| y.1 .1.partial_cmp(&x.1 .1).unwrap());
    for (c, (n, ms)) in v {
        eprintln!("{c:28} {n:4} calls {ms:9.0} ms");
    }
    let mut slow: Vec<_> = h.log.iter().enumerate().collect();
    slow.sort_by(|x, y| y.1 .1.partial_cmp(&x.1 .1).unwrap());
    for (i, (c, ms)) in slow.iter().take(8) {
        eprintln!("  slowest: #{i} {c} {ms:.0} ms");
    }
    eprintln!("wall {wall:.1}s, {} calls", h.log.len());
    if let Ok(path) = std::env::var("CALL_LOG") {
        let lines: Vec<String> = h.log.iter().enumerate().map(|(i, (c, ms))| format!("{i}\t{c}\t{ms:.0}")).collect();
        std::fs::write(path, lines.join("\n")).unwrap();
    }
    match r {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => println!("ERROR: {e}"),
    }
    if let Some(out) = a.get(4) {
        let _ = h.tool("save", json!({"path": out}));
    }
}
