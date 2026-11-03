use crate::player::{Player, PlayerStatus};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

// Exercise the same audio engine and bundled-tool lookup as the GUI, not a mock player.
pub fn run(source: &str, report: &Path) -> Result<()> {
    let result = check(source);
    let body = match &result {
        Ok(value) => value.clone(),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    };
    std::fs::write(report, serde_json::to_vec_pretty(&body)?)
        .context("Writing playback test report")?;
    result.map(|_| ())
}

fn wait_for(
    player: &mut Player,
    timeout: u64,
    label: &str,
    condition: impl Fn(PlayerStatus) -> bool,
) -> Result<PlayerStatus> {
    let until = Instant::now() + Duration::from_secs(timeout);
    loop {
        let state = player
            .poll()
            .ok_or_else(|| anyhow!("Player exited during {label}"))?;
        if let Some(error) = player.take_error() {
            bail!("mpv command failed during {label}: {error}");
        }
        if state.errored {
            bail!("Audio decoding or output failed during {label}");
        }
        if condition(state) && state.pending_commands == 0 {
            return Ok(state);
        }
        if state.ended {
            bail!("Track ended before {label} could be checked");
        }
        if Instant::now() >= until {
            bail!("Timed out during {label}: {state:?}");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn check(source: &str) -> Result<Value> {
    let mut player = Player::new();
    let audio = crate::store::Library::load(&crate::store::data_path())?.audio;
    player.load_with_audio(source, 15.0, 1.0, &audio)?;
    let initial = wait_for(&mut player, 60, "decoding and audio output", |s| {
        s.position > 0.1 && s.duration > 2.0 && s.audio_sample_rate > 0
    })?;
    if player.audio_backend().is_empty() {
        bail!("No audio output backend was opened");
    }
    let backend = player.audio_backend().to_owned();
    player.toggle()?;
    wait_for(&mut player, 5, "pause", |s| s.paused)?;
    let target = (initial.duration * 0.4).min(1.5);
    player.seek(target)?;
    wait_for(&mut player, 10, "seek", |s| {
        s.paused && (s.position - target).abs() < 0.4
    })?;
    player.volume(25.0)?;
    player.speed(1.25)?;
    wait_for(&mut player, 5, "volume and speed", |s| {
        (s.volume - 25.0).abs() < 0.1 && (s.speed - 1.25).abs() < 0.01
    })?;
    player.toggle()?;
    let final_state = wait_for(&mut player, 10, "resume and advancing audio", |s| {
        !s.paused && s.position > target + 0.25
    })?;
    player.stop();
    Ok(
        json!({"ok":true,"audio_backend":backend,"sample_rate":initial.audio_sample_rate,
        "duration":initial.duration,"position_after_resume":final_state.position,"volume":final_state.volume,"speed":final_state.speed,
        "checks":["decode","audio output","pause","seek","volume","speed","resume","stop"]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_is_reported_instead_of_claiming_playback() {
        let report = std::env::temp_dir().join(format!(
            "bitchord-playback-error-{}.json",
            std::process::id()
        ));
        assert!(run("not-a-real-audio-file", &report).is_err());
        let value: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value["error"]
            .as_str()
            .unwrap()
            .contains("existing local file"));
        std::fs::remove_file(report).unwrap();
    }
}
