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

    /// 从 kiro-cli 的 SQLite 数据库导入凭据到 credentials.json
    /// 默认读取 ~/.local/share/kiro-cli/data.sqlite3
    #[arg(long)]
    pub import_kiro_cli: bool,

    /// kiro-cli 数据库路径（配合 --import-kiro-cli 使用）
    #[arg(long)]
    pub kiro_cli_db: Option<String>,
}
