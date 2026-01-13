//! 调试工具模块
//!
//! 提供 hex 打印和 CRC 调试等功能

use crate::kiro::model::events::Event;
use std::io::Write;

/// 打印 hex 数据 (类似 xxd 格式)
pub fn print_hex(data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        // 打印偏移
        print!("{:08x}: ", i * 16);

        // 打印 hex
        for (j, byte) in chunk.iter().enumerate() {
            if j == 8 {
                print!(" ");
            }
            print!("{:02x} ", byte);
        }

        // 补齐空格
        let padding = 16 - chunk.len();
        for j in 0..padding {
            if chunk.len() + j == 8 {
                print!(" ");
            }
            print!("   ");
        }

        // 打印 ASCII
        print!(" |");
        for byte in chunk {
            if *byte >= 0x20 && *byte < 0x7f {
                print!("{}", *byte as char);
            } else {
                print!(".");
            }
        }
        println!("|");
    }
    std::io::stdout().flush().ok();
}

/// 调试 CRC 计算 - 分析 AWS Event Stream 帧的 CRC
pub fn debug_crc(data: &[u8]) {
    if data.len() < 12 {
        println!("[CRC 调试] 数据不足 12 字节");
        return;
    }

    use crc::{Crc, CRC_32_BZIP2, CRC_32_ISO_HDLC, CRC_32_ISCSI, CRC_32_JAMCRC};

    let total_length = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let header_length = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let prelude_crc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    println!("\n[CRC 调试]");
    println!("  total_length: {} (0x{:08x})", total_length, total_length);
    println!(
        "  header_length: {} (0x{:08x})",
        header_length, header_length
    );
    println!("  prelude_crc (from data): 0x{:08x}", prelude_crc);

    // 测试各种 CRC32 变种
    let crc32c: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);
    let crc32_iso: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
    let crc32_bzip2: Crc<u32> = Crc::<u32>::new(&CRC_32_BZIP2);
    let crc32_jamcrc: Crc<u32> = Crc::<u32>::new(&CRC_32_JAMCRC);

    let prelude = &data[..8];

    println!("  CRC32C (ISCSI):   0x{:08x}", crc32c.checksum(prelude));
    println!(
        "  CRC32 ISO-HDLC:   0x{:08x} {}",
        crc32_iso.checksum(prelude),
        if crc32_iso.checksum(prelude) == prelude_crc {
            "<-- MATCH"
        } else {
            ""
        }
    );
    println!("  CRC32 BZIP2:      0x{:08x}", crc32_bzip2.checksum(prelude));
    println!(
        "  CRC32 JAMCRC:     0x{:08x}",
        crc32_jamcrc.checksum(prelude)
    );

    // 打印前 8 字节
    print!("  前 8 字节: ");
    for byte in prelude {
        print!("{:02x} ", byte);
    }
    println!();
}

/// 打印帧摘要信息
pub fn print_frame_summary(data: &[u8]) {
    if data.len() < 12 {
        println!("[帧摘要] 数据不足");
        return;
    }

    let total_length = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let header_length = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;

    println!("\n[帧摘要]");
    println!("  总长度: {} 字节", total_length);
    println!("  头部长度: {} 字节", header_length);
    println!("  Payload 长度: {} 字节", total_length.saturating_sub(12 + header_length + 4));
    println!("  数据可用: {} 字节", data.len());

    if data.len() >= total_length {
        println!("  状态: 完整帧");
    } else {
        println!(
            "  状态: 不完整 (缺少 {} 字节)",
            total_length - data.len()
        );
    }
}

/// 详细打印事件 (调试格式，包含事件类型和完整数据)
pub fn print_event_verbose(event: &Event) {
    match event {
        Event::AssistantResponse(e) => {
            println!("\n[事件] AssistantResponse");
            println!("  content: {:?}", e.content());
        }
        Event::ToolUse(e) => {
            println!("\n[事件] ToolUse");
            println!("  name: {:?}", e.name());
            println!("  tool_use_id: {:?}", e.tool_use_id());
            println!("  input: {:?}", e.input());
            println!("  stop: {}", e.is_complete());
        }
        Event::Metering(e) => {
            println!("\n[事件] Metering");
            println!("  unit: {:?}", e.unit);
            println!("  unit_plural: {:?}", e.unit_plural);
            println!("  usage: {}", e.usage);
        }
        Event::ContextUsage(e) => {
            println!("\n[事件] ContextUsage");
            println!("  context_usage_percentage: {}", e.context_usage_percentage);
        }
        Event::Unknown { event_type, payload } => {
            println!("\n[事件] Unknown");
            println!("  event_type: {:?}", event_type);
            println!("  payload ({} bytes):", payload.len());
            print_hex(payload);
        }
        Event::Error {
            error_code,
            error_message,
        } => {
            println!("\n[事件] Error");
            println!("  error_code: {:?}", error_code);
            println!("  error_message: {:?}", error_message);
        }
        Event::Exception {
            exception_type,
            message,
        } => {
            println!("\n[事件] Exception");
            println!("  exception_type: {:?}", exception_type);
            println!("  message: {:?}", message);
        }
    }
}

/// 简洁打印事件 (用于正常输出)
pub fn print_event(event: &Event) {
    match event {
        Event::AssistantResponse(e) => {
            // 实时打印助手响应，不换行
            print!("{}", e.content());
            std::io::stdout().flush().ok();
        }
        Event::ToolUse(e) => {
            println!("\n[工具调用] {} (id: {})", e.name(), e.tool_use_id());
            println!("  输入: {}", e.input());
            if e.is_complete() {
                println!("  [调用结束]");
            }
        }
        Event::Metering(e) => {
            println!("\n[计费] {}", e);
        }
        Event::ContextUsage(e) => {
            println!("\n[上下文使用率] {}", e);
        }
        Event::Unknown { event_type, .. } => {
            println!("\n[未知事件] {}", event_type);
        }
        Event::Error {
            error_code,
            error_message,
        } => {
            println!("\n[错误] {}: {}", error_code, error_message);
        }
        Event::Exception {
            exception_type,
            message,
        } => {
            println!("\n[异常] {}: {}", exception_type, message);
        }
    }
}
