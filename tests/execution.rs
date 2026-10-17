use sm0sh::{command::CommandSpec, config::Config, executor::execute, memory::Status};
use std::time::{Duration, Instant};
use tokio::sync::watch;

fn shell(script: &str) -> CommandSpec {
    CommandSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
    }
}

#[tokio::test]
async fn stdout_stderr_and_exit_code_are_captured() {
    let (_sender, stop) = watch::channel(false);
    let result = execute(
        &shell("printf 'out'; printf 'err' >&2; exit 7"),
        &Config::default(),
        stop,
    )
    .await;
    assert_eq!(result.status, Status::ExitFailure);
    assert_eq!(result.exit_code, Some(7));
    assert_eq!(result.text, "outerr");
}

#[tokio::test]
async fn arguments_are_literal_without_shell_expansion() {
    let (_sender, stop) = watch::channel(false);
    let spec = CommandSpec {
        program: "/usr/bin/printf".into(),
        args: vec![
            "%s|%s|%s".into(),
            "".into(),
            "a b".into(),
            "$(echo nope);日本語".into(),
        ],
    };
    let result = execute(&spec, &Config::default(), stop).await;
    assert_eq!(result.status, Status::Success);
    assert_eq!(result.text, "|a b|$(echo nope);日本語");
}

#[tokio::test]
async fn missing_executable_has_a_diagnostic() {
    let (_sender, stop) = watch::channel(false);
    let spec = CommandSpec {
        program: "/does-not-exist/sm0sh-test".into(),
        args: vec![],
    };
    let result = execute(&spec, &Config::default(), stop).await;
    assert_eq!(result.status, Status::SpawnFailure);
    assert!(result.text.contains("sm0sh-test"));
}

#[tokio::test]
async fn large_output_is_drained_with_a_bounded_capture() {
    let (_sender, stop) = watch::channel(false);
    let config = Config {
        sd_words: 32,
        ..Config::default()
    };
    let result = execute(
        &shell("head -c 1000000 /dev/zero | tr '\\000' x"),
        &config,
        stop,
    )
    .await;
    assert_eq!(result.status, Status::Success);
    assert_eq!(result.text, "x".repeat(63));
    assert!(result.truncated);
}

#[tokio::test]
async fn timeout_and_service_stop_terminate_jobs() {
    let (sender, stop) = watch::channel(false);
    let config = Config {
        command_timeout_ms: 150,
        ..Config::default()
    };
    let start = Instant::now();
    let result = execute(&shell("printf started; sleep 30"), &config, stop.clone()).await;
    assert_eq!(result.status, Status::Timeout);
    assert!(result.text.starts_with("started"));
    assert!(start.elapsed() < Duration::from_secs(3));
    let job =
        tokio::spawn(async move { execute(&shell("sleep 30"), &Config::default(), stop).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    sender.send(true).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, Status::Cancelled);
}

#[tokio::test]
async fn descendants_do_not_keep_output_open_after_parent_exit() {
    let (_sender, stop) = watch::channel(false);
    let start = Instant::now();
    let result = execute(
        &shell("sleep 30 & printf '%s' $!"),
        &Config::default(),
        stop,
    )
    .await;
    assert_eq!(result.status, Status::Success);
    assert!(start.elapsed() < Duration::from_secs(3));
    let pid: u32 = result.text.trim().parse().unwrap();
    // A killed orphan can briefly remain running or as a zombie until PID 1 reaps it.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                let state = stat.split_once(") ").unwrap().1;
                if state.starts_with('Z') {
                    break;
                }
                if Instant::now() >= deadline {
                    panic!("child is still running: {stat}");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(_) => break,
        }
    }
}
