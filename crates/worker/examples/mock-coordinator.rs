//! A stand-alone mock coordinator for end-to-end runs of the worker binary:
//!
//! ```text
//! cargo run -p ibc-worker --release --example mock-coordinator -- --jobs 20 --port 8787
//! ibc-worker --coordinator http://127.0.0.1:8787 --registration-token mock --runner $IBC_RUNNER --once
//! ```
//!
//! It queues `--jobs` real matches (rusher vs holder; jobs 2k and 2k+1 are identical, so their replays must
//! be), waits until every job is completed or failed, prints a report and exits 0 only if all completed and
//! every duplicated pair agrees on `replay_sha256`.

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::time::Duration;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let jobs: u64 = arg("--jobs")
        .map(|v| v.parse().expect("--jobs N"))
        .unwrap_or(20);
    let port: u16 = arg("--port")
        .map(|v| v.parse().expect("--port P"))
        .unwrap_or(8787);
    let token = arg("--registration-token").unwrap_or_else(|| "mock".into());
    let root = arg("--repo")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let runner = arg("--runner")
        .or_else(|| std::env::var("IBC_RUNNER").ok())
        .expect("--runner PATH or IBC_RUNNER (to learn the patch)");
    let patch = serde_json::to_value(
        ibc_worker::runner::probe_version(runner.as_ref()).expect("ibc version --json"),
    )
    .unwrap();
    let read = |rel: &str| {
        std::fs::read(root.join(rel)).unwrap_or_else(|e| {
            panic!("{rel}: {e} (build the bots with `ibc build bots/<name>`; --repo sets the root)")
        })
    };

    let mock = common::Mock::start(port, &token);
    let bot_a = mock.add_blob(&read("bots/rusher/.ibc/bot.wasm"));
    let bot_b = mock.add_blob(&read("bots/holder/.ibc/bot.wasm"));
    let maps = [
        mock.add_blob(&read("maps/crossfire.ibcmap")),
        mock.add_blob(&read("maps/tunnels.ibcmap")),
    ];
    for i in 0..jobs {
        let pair = i / 2;
        mock.push_job(common::match_job(
            &format!("job-{i:04}"),
            &patch,
            &bot_a,
            &bot_b,
            &maps[(pair % 2) as usize],
            pair + 1,
        ));
    }
    println!(
        "mock coordinator on {} (registration token {token:?}), {jobs} jobs queued, patch {patch}",
        mock.url
    );

    let mut last = 0;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let st = mock.state();
        let done = st.completed.len() + st.failed.len();
        if done != last && (done % 100 == 0 || done as u64 == jobs) {
            println!(
                "{done}/{jobs} reported, {} queued, {} workers registered",
                st.queue.len(),
                st.registrations
            );
        }
        last = done;
        if done as u64 >= jobs {
            break;
        }
    }

    // Stay up for a moment so a `--once` worker sees an empty queue and exits on its own.
    std::thread::sleep(Duration::from_secs(3));
    let st = mock.state();
    let sha = |id: &str| {
        st.completed
            .iter()
            .find(|(j, _)| j == id)
            .map(|(_, r)| r["replay_sha256"].as_str().unwrap_or("").to_string())
    };
    let (mut pairs, mut agree) = (0, 0);
    for k in 0..jobs / 2 {
        pairs += 1;
        let (a, b) = (
            sha(&format!("job-{:04}", 2 * k)),
            sha(&format!("job-{:04}", 2 * k + 1)),
        );
        if a.is_some() && a == b {
            agree += 1;
        } else {
            println!("MISMATCH pair {k}: {a:?} vs {b:?}");
        }
    }
    let distinct: std::collections::HashSet<_> = st
        .completed
        .iter()
        .map(|(_, r)| r["replay_sha256"].to_string())
        .collect();
    let elapsed = match (st.first_lease, st.last_report) {
        (Some(a), Some(b)) => b.duration_since(a).as_secs_f64(),
        _ => 0.0,
    };
    println!("completed {} failed {} of {jobs}; {agree}/{pairs} duplicated pairs have identical replay_sha256; {} distinct replays", st.completed.len(), st.failed.len(), distinct.len());
    println!(
        "first lease to last report: {elapsed:.2} s = {:.1} matches/s; heartbeats seen: {}",
        st.completed.len() as f64 / elapsed.max(1e-9),
        st.heartbeats.values().sum::<u32>()
    );
    for (id, error, retryable) in &st.failed {
        println!("FAILED {id}: {error} (retryable: {retryable})");
    }
    std::process::exit(if st.completed.len() as u64 == jobs && agree == pairs {
        0
    } else {
        1
    });
}
