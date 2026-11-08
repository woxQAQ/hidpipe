//! Explicit, loopback-only input source for the real macOS client.
use hidpipe_proto::{read_frame, write_frame, Frame, VERSION};
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::thread;
use std::time::{Duration, Instant};

const HELP: &str = "scroll_fixture: WILL SCROLL THE CURRENT APPLICATION after READY + Enter.
Usage: cargo run -p hidpipe-proto --example scroll_fixture -- [options]
  --port <1..65535>       loopback port (default 24802)
  --case <name>           default whole; sequences (raw units, 120/notch):
    whole [120]; quarters [30,30,30,30]; two-whole [120,120]; double [240];
    tiny [1,119]; reverse [30,-30]; negative [-30,-30,-30,-30]
  --axis <x|y>           default y
  --interval-ms <1..10000> default 20; also compare 5 and 100
  --start-delay-ms <0..10000> default 0; use 2000 to focus the target app after Enter
  --timeout-seconds <1..3600> accept/READY/sequence deadlines (default 120 each)
  --help                 print help without binding a socket
One client, native Wheel only, no device capture. Requires Hello VERSION/openlogi=false.
Start listener after READY; press Enter only after it prints LISTENING.
Ping every 250 ms; socket I/O timeout 3 s; replies are drained in a background thread.
";

struct Options {
    port: u16,
    case: String,
    values: &'static [i32],
    axis: String,
    interval: Duration,
    start_delay: Duration,
    timeout: Duration,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn options() -> io::Result<Option<Options>> {
    let mut args = std::env::args().skip(1);
    let mut opt = Options {
        port: 24802,
        case: "whole".into(),
        values: &[120],
        axis: "y".into(),
        interval: Duration::from_millis(20),
        start_delay: Duration::ZERO,
        timeout: Duration::from_secs(120),
    };
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            print!("{HELP}");
            return Ok(None);
        }
        if !matches!(
            flag.as_str(),
            "--port"
                | "--case"
                | "--axis"
                | "--interval-ms"
                | "--start-delay-ms"
                | "--timeout-seconds"
        ) {
            return Err(invalid(format!("unknown option {flag}")));
        }
        let value = args
            .next()
            .ok_or_else(|| invalid(format!("missing {flag} value")))?;
        match flag.as_str() {
            "--case" => opt.case = value,
            "--axis" if value == "x" || value == "y" => opt.axis = value,
            "--axis" => return Err(invalid("axis must be x or y")),
            _ => {
                let n: u64 = value
                    .parse()
                    .map_err(|_| invalid(format!("invalid {flag}")))?;
                let max = match flag.as_str() {
                    "--port" => 65535,
                    "--interval-ms" | "--start-delay-ms" => 10000,
                    _ => 3600,
                };
                let min = u64::from(flag != "--start-delay-ms");
                if n < min || n > max {
                    return Err(invalid(format!("{flag} must be {min}..{max}")));
                }
                match flag.as_str() {
                    "--port" => opt.port = n as u16,
                    "--interval-ms" => opt.interval = Duration::from_millis(n),
                    "--start-delay-ms" => opt.start_delay = Duration::from_millis(n),
                    _ => opt.timeout = Duration::from_secs(n),
                }
            }
        }
    }
    opt.values = match opt.case.as_str() {
        "whole" => &[120],
        "quarters" => &[30, 30, 30, 30],
        "two-whole" => &[120, 120],
        "double" => &[240],
        "tiny" => &[1, 119],
        "reverse" => &[30, -30],
        "negative" => &[-30, -30, -30, -30],
        _ => return Err(invalid("unknown case; see --help")),
    };
    Ok(Some(opt))
}

struct Session {
    stream: TcpStream,
    errors: mpsc::Receiver<String>,
    next_ping: Instant,
}

impl Session {
    fn tick(&mut self, deadline: Instant) -> io::Result<()> {
        if let Ok(error) = self.errors.try_recv() {
            return Err(io::Error::other(error));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "stage deadline expired",
            ));
        }
        if Instant::now() >= self.next_ping {
            write_frame(&mut self.stream, &Frame::Ping)?;
            self.next_ping = Instant::now() + Duration::from_millis(250);
        }
        Ok(())
    }

    fn wait(&mut self, duration: Duration, deadline: Instant) -> io::Result<()> {
        let until = Instant::now() + duration;
        loop {
            self.tick(deadline)?;
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }
}

fn run(opt: Options) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", opt.port))?;
    listener.set_nonblocking(true)?;
    println!(
        "BOUND 127.0.0.1:{} VERSION={VERSION}; waiting for one client",
        opt.port
    );
    io::stdout().flush()?;
    let deadline = Instant::now() + opt.timeout;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "accept timeout"));
            }
            Err(e) => return Err(e),
        }
    };
    drop(listener);
    // BSD/macOS can inherit the listener's nonblocking mode; socket timeouts require blocking I/O.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    if read_frame(&mut stream)?
        != (Frame::Hello {
            version: VERSION,
            openlogi: false,
        })
    {
        return Err(invalid(
            "expected Hello { version: VERSION, openlogi: false }",
        ));
    }

    let mut reader = stream.try_clone()?;
    let closing = Arc::new(AtomicBool::new(false));
    let reader_closing = closing.clone();
    let pongs = Arc::new(AtomicUsize::new(0));
    let returns = Arc::new(AtomicUsize::new(0));
    let (reader_pongs, reader_returns) = (pongs.clone(), returns.clone());
    let (errors, error_rx) = mpsc::channel();
    let drain = thread::spawn(move || loop {
        let error = match read_frame(&mut reader) {
            Ok(Frame::Pong) => {
                reader_pongs.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            Ok(Frame::ReturnToLocal) => {
                reader_returns.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            Ok(frame) => format!("unexpected client frame: {frame:?}"),
            Err(_) if reader_closing.load(Ordering::SeqCst) => break,
            Err(e) => format!("client reply stream failed: {e}"),
        };
        let _ = errors.send(error);
        break;
    });
    let mut session = Session {
        stream,
        errors: error_rx,
        next_ping: Instant::now(),
    };
    let result = (|| {
        println!(
            "READY case={} axis={} values={:?} total={} interval_ms={}; start PID listener, then press Enter (will scroll current app)",
            opt.case, opt.axis, opt.values, opt.values.iter().sum::<i32>(), opt.interval.as_millis()
        );
        io::stdout().flush()?;
        let (input, entered) = mpsc::channel();
        // A blocked terminal read never blocks socket cleanup or process exit.
        thread::spawn(move || {
            let mut line = String::new();
            let result = match io::stdin().read_line(&mut line) {
                Ok(_) if line.ends_with('\n') => Ok(()),
                Ok(_) => Err("stdin EOF before Enter".to_string()),
                Err(e) => Err(format!("stdin: {e}")),
            };
            let _ = input.send(result);
        });
        let deadline = Instant::now() + opt.timeout;
        loop {
            session.tick(deadline)?;
            match entered.try_recv() {
                Ok(result) => {
                    result.map_err(io::Error::other)?;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::other("stdin worker ended"))
                }
                Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(10)),
            }
        }
        let deadline = Instant::now() + opt.timeout;
        println!(
            "START delay_ms={}; focus the target application now",
            opt.start_delay.as_millis()
        );
        io::stdout().flush()?;
        session.wait(opt.start_delay, deadline)?;
        for (index, &value) in opt.values.iter().enumerate() {
            session.tick(deadline)?;
            let (dx, dy) = if opt.axis == "x" {
                (value, 0)
            } else {
                (0, value)
            };
            write_frame(&mut session.stream, &Frame::Wheel { dx, dy })?;
            println!("SEND #{} dx={dx} dy={dy}", index + 1);
            if index + 1 < opt.values.len() {
                session.wait(opt.interval, deadline)?;
            }
        }
        write_frame(&mut session.stream, &Frame::ReleaseAll)?;
        // Allow ordered feedback to drain before our intentional disconnect.
        session.wait(Duration::from_millis(300), deadline)?;
        Ok(())
    })();
    closing.store(true, Ordering::SeqCst);
    let shutdown = session.stream.shutdown(Shutdown::Both);
    drain
        .join()
        .map_err(|_| io::Error::other("reply worker panicked"))?;
    result?;
    if let Ok(error) = session.errors.try_recv() {
        return Err(io::Error::other(error));
    }
    shutdown?;
    println!("COMPLETE sent={} total={} pongs={} return_to_local={} (replies are not inputs or injection evidence)",
        opt.values.len(), opt.values.iter().sum::<i32>(), pongs.load(Ordering::Relaxed), returns.load(Ordering::Relaxed));
    Ok(())
}

fn main() -> io::Result<()> {
    if let Some(opt) = options()? {
        run(opt)?;
    }
    Ok(())
}
