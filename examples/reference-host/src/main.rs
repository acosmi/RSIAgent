//! Read-only reference host probe for the first G1 task: locate configuration
//! according to a fixed precedence order. This standalone file intentionally
//! is not a workspace crate; compile it directly with `rustc --edition 2024`.

use std::{env, path::PathBuf, process::ExitCode};
mod curriculum_target;

fn main() -> ExitCode {
    let mut explicit = None;
    let mut task = "config_precedence".to_string();
    let mut clamp_value = None;
    let mut clamp_min = None;
    let mut clamp_max = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => explicit = args.next().map(PathBuf::from),
            "--task" => task = args.next().unwrap_or_default(),
            "--value" => clamp_value = args.next().and_then(|value| value.parse::<i64>().ok()),
            "--min" => clamp_min = args.next().and_then(|value| value.parse::<i64>().ok()),
            "--max" => clamp_max = args.next().and_then(|value| value.parse::<i64>().ok()),
            _ => {
                println!("{{\"status\":\"invalid_input\",\"reason\":\"unknown_argument\"}}");
                return ExitCode::from(2);
            }
        }
    }
    if task == "code" {
        println!(
            "{{\"task\":\"code\",\"status\":\"sandbox_unavailable\",\"writes_performed\":false}}"
        );
        return ExitCode::from(3);
    }
    if task == "pure_clamp_i64" {
        let (Some(value), Some(min), Some(max)) = (clamp_value, clamp_min, clamp_max) else {
            println!("{{\"status\":\"invalid_input\",\"reason\":\"missing_clamp_fields\"}}");
            return ExitCode::from(2);
        };
        return match curriculum_target::clamp_i64(curriculum_target::ClampInput { value, min, max })
        {
            Ok(clamped) => {
                println!(
                    "{{\"task\":\"pure_clamp_i64\",\"target_id\":\"{}\",\"status\":\"offline_target_only\",\"clamped\":{},\"sandbox_verified\":false,\"writes_performed\":false}}",
                    curriculum_target::TARGET_ID,
                    clamped
                );
                ExitCode::SUCCESS
            }
            Err(reason) => {
                println!(
                    "{{\"task\":\"pure_clamp_i64\",\"target_id\":\"{}\",\"status\":\"invalid_input\",\"reason\":\"{}\",\"sandbox_verified\":false,\"writes_performed\":false}}",
                    curriculum_target::TARGET_ID,
                    reason
                );
                ExitCode::from(2)
            }
        };
    }
    if task != "config_precedence" {
        println!("{{\"status\":\"unsupported\",\"reason\":\"unsupported_reference_task\"}}");
        return ExitCode::from(2);
    }

    let cwd = match env::current_dir() {
        Ok(path) => path,
        Err(_) => {
            println!("{{\"status\":\"io_error\",\"writes_performed\":false}}");
            return ExitCode::from(1);
        }
    };
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(("cli", path));
    }
    if let Some(path) = env::var_os("RSIA_REFERENCE_CONFIG") {
        candidates.push(("environment", PathBuf::from(path)));
    }
    candidates.push(("workspace_dot_rsia", cwd.join(".rsia/config.json")));
    candidates.push(("workspace_root", cwd.join("rsia.json")));

    let selected = candidates.iter().find(|(_, path)| path.is_file());
    match selected {
        Some((source, path)) => println!(
            "{{\"task\":\"config_precedence\",\"status\":\"ok\",\"selected_source\":\"{}\",\"selected_path\":\"{}\",\"writes_performed\":false}}",
            escape(source),
            escape(&path.to_string_lossy())
        ),
        None => println!(
            "{{\"task\":\"config_precedence\",\"status\":\"not_found\",\"checked_count\":{},\"writes_performed\":false}}",
            candidates.len()
        ),
    }
    ExitCode::SUCCESS
}

fn escape(value: &str) -> String {
    use std::fmt::Write as _;

    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control <= '\u{1f}' => {
                write!(&mut escaped, "\\u{:04x}", control as u32)
                    .expect("writing to String cannot fail");
            }
            other => escaped.push(other),
        }
    }
    escaped
}
