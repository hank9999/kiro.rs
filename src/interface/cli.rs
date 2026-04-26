//! CLI 参数定义
//!
//! 单独成模块让 `main.rs` 退化为 wiring 入口，CLI 协议（flags、help 文本、
//! parser 行为）的回归测试可以脱离 main 函数独立运行。

use clap::Parser;

/// Anthropic <-> Kiro API 客户端
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// 配置文件路径
    #[arg(short, long)]
    pub config: Option<String>,

    /// 凭证文件路径
    #[arg(long)]
    pub credentials: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不传任何 flag 时两个字段都应为 None。
    #[test]
    fn default_args_parses_with_no_flags() {
        let args = Args::try_parse_from(["binary"]).expect("解析失败");
        assert!(args.config.is_none());
        assert!(args.credentials.is_none());
    }

    /// `--config` 与 `--credentials` 应分别填入对应字段。
    #[test]
    fn custom_paths_parse_correctly() {
        let args = Args::try_parse_from([
            "binary",
            "--config",
            "foo.json",
            "--credentials",
            "bar.json",
        ])
        .expect("解析失败");
        assert_eq!(args.config.as_deref(), Some("foo.json"));
        assert_eq!(args.credentials.as_deref(), Some("bar.json"));
    }

    /// `-c` 短 flag 必须等价于 `--config`，确保 derive Parser 配置不会被无意改动。
    #[test]
    fn short_config_flag_is_equivalent_to_long() {
        let args = Args::try_parse_from(["binary", "-c", "alt.json"]).expect("解析失败");
        assert_eq!(args.config.as_deref(), Some("alt.json"));
        assert!(args.credentials.is_none());
    }
}
