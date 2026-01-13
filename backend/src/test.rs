use std::sync::Arc;
use futures::StreamExt;

use crate::debug::{print_event, print_event_verbose, debug_crc, print_hex};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::events::Event;
use crate::kiro::model::requests::KiroRequest;
use crate::kiro::parser::EventStreamDecoder;
use crate::kiro::provider::KiroProvider;
use crate::kiro::token_manager::MultiTokenManager;
use crate::model::config::Config;


/// 调用流式 API 并实时打印返回
pub(crate) async fn call_stream_api() -> anyhow::Result<()> {
    // 读取 test.json 作为请求体
    let request_body = std::fs::read_to_string("test.json")?;
    println!("已加载请求体，长度: {} 字节", request_body.len());

    // 解析请求体为 KiroRequest 对象
    let request: KiroRequest = serde_json::from_str(&request_body)?;
    println!("已解析请求对象:");
    println!("  会话 ID: {}", request.conversation_id());
    println!("  模型 ID: {}", request.model_id());
    println!("  消息内容长度: {} 字符", request.current_content().len());
    if let Some(ref task_type) = request.conversation_state.agent_task_type {
        println!("  任务类型: {}", task_type);
    }
    if let Some(ref trigger_type) = request.conversation_state.chat_trigger_type {
        println!("  触发类型: {}", trigger_type);
    }
    println!("  历史消息数: {}", request.conversation_state.history.len());
    println!("  工具数量: {}", request.conversation_state.current_message.user_input_message.user_input_message_context.tools.len());

    // 加载凭证
    let credentials = KiroCredentials::load_default()?;
    println!("已加载凭证");

    // 加载配置
    let config = Config::load(Config::default_config_path())?;
    println!("已加载配置");

    // 创建 MultiTokenManager 和 KiroProvider
    let token_manager = MultiTokenManager::new(config, vec![credentials], None)?;
    let provider = KiroProvider::new(Arc::new(token_manager));

    println!("\n开始调用流式 API...\n");
    println!("{}", "=".repeat(60));

    // 调用流式 API
    let response = provider.call_api_stream(&request_body).await?;

    // 获取字节流
    let mut stream = response.bytes_stream();
    let mut decoder = EventStreamDecoder::new();

    // 处理流式数据
    let mut total_bytes = 0usize;
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                // 调试模式：打印原始 hex 数据
                // println!("\n[收到数据块] {} 字节, 偏移 {}", chunk.len(), total_bytes);
                // print_hex(&chunk);
                // debug_crc(&chunk);

                total_bytes += chunk.len();

                // 将数据喂给解码器
                if let Err(e) = decoder.feed(&chunk) {
                    eprintln!("[缓冲区错误] {}", e);
                    continue;
                }

                // 解码所有可用的帧
                for result in decoder.decode_iter() {
                    match result {
                        Ok(frame) => {
                            // 解析事件
                            match Event::from_frame(frame) {
                                Ok(event) => {
                                    // 简洁输出
                                    // print_event(&event);
                                    // 详细输出 (调试用)
                                    print_event_verbose(&event);
                                }
                                Err(e) => eprintln!("[解析错误] {}", e),
                            }
                        }
                        Err(e) => {
                            eprintln!("[帧解析错误] {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[网络错误] {}", e);
                break;
            }
        }
    }

    println!("\n{}", "=".repeat(60));
    println!("流式响应结束");
    println!("共接收 {} 字节，解码 {} 帧", total_bytes, decoder.frames_decoded());

    Ok(())
}