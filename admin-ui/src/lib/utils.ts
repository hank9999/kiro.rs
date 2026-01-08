import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * 解析后端错误响应，提取用户友好的错误信息
 */
export interface ParsedError {
  /** 简短的错误标题 */
  title: string
  /** 详细的错误描述 */
  detail?: string
  /** 错误类型 */
  type?: string
}

/**
 * 从错误对象中提取错误消息
 * 支持 Axios 错误和普通 Error 对象
 */
export function extractErrorMessage(error: unknown): string {
  const parsed = parseError(error)
  return parsed.title
}

/**
 * 解析错误，返回结构化的错误信息
 */
export function parseError(error: unknown): ParsedError {
  if (!error || typeof error !== 'object') {
    return { title: '未知错误' }
  }

  const axiosError = error as Record<string, unknown>
  const response = axiosError.response as Record<string, unknown> | undefined
  const data = response?.data as Record<string, unknown> | undefined
  const errorObj = data?.error as Record<string, unknown> | undefined

  // 尝试从后端错误响应中提取信息
  if (errorObj && typeof errorObj.message === 'string') {
    const message = errorObj.message
    const type = typeof errorObj.type === 'string' ? errorObj.type : undefined

    // 解析嵌套的错误信息（如：上游服务错误: 权限不足: 403 {...}）
    const parsed = parseNestedErrorMessage(message)

    return {
      title: parsed.title,
      detail: parsed.detail,
      type,
    }
  }

  // 回退到 Error.message
  if ('message' in axiosError && typeof axiosError.message === 'string') {
    return { title: axiosError.message }
  }

  return { title: '未知错误' }
}

/**
 * 解析嵌套的错误消息
 * 例如："上游服务错误: 权限不足，无法获取使用额度: 403 Forbidden {...}"
 */
function parseNestedErrorMessage(message: string): { title: string; detail?: string } {
  // 尝试提取 HTTP 状态码（如 403、502 等）
  const statusMatch = message.match(/(\d{3})\s+\w+/)
  const statusCode = statusMatch ? statusMatch[1] : null

  // 尝试提取 JSON 中的 message 字段
  const jsonMatch = message.match(/\{[^{}]*"message"\s*:\s*"([^"]+)"[^{}]*\}/)
  if (jsonMatch) {
    const innerMessage = jsonMatch[1]
    // 提取主要错误原因（去掉前缀）
    const parts = message.split(':').map(s => s.trim())
    const mainReason = parts.length > 1 ? parts[1].split(':')[0] : parts[0]

    // 在 title 中包含状态码
    const title = statusCode
      ? `${mainReason || '服务错误'} (${statusCode})`
      : (mainReason || '服务错误')

    return {
      title,
      detail: innerMessage,
    }
  }

  // 尝试按冒号分割，提取主要信息
  const colonParts = message.split(':')
  if (colonParts.length >= 2) {
    const mainPart = colonParts[1].trim().split(':')[0].trim()
    const title = statusCode ? `${mainPart} (${statusCode})` : mainPart

    return {
      title,
      detail: colonParts.slice(2).join(':').trim() || undefined,
    }
  }

  return { title: message }
}
