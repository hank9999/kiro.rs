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




/**
 * 计算字符串的 SHA-256 哈希（十六进制）
 *
 * 优先使用 Web Crypto API（crypto.subtle），在非安全上下文（HTTP + 非 localhost）中
 * 自动回退到纯 JS 实现，解决 Docker 部署时 crypto.subtle 不可用的问题。
 */
export async function sha256Hex(value: string): Promise<string> {
  const encoded = new TextEncoder().encode(value)

  // 安全上下文中使用原生 Web Crypto API（性能更好）
  if (typeof crypto !== 'undefined' && crypto.subtle) {
    const digest = await crypto.subtle.digest('SHA-256', encoded)
    const bytes = new Uint8Array(digest)
    return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('')
  }

  // 非安全上下文 fallback：纯 JS SHA-256 实现
  return sha256Pure(encoded)
}

/**
 * 纯 JS SHA-256 实现（无外部依赖）
 * 仅在 crypto.subtle 不可用时使用
 */
function sha256Pure(data: Uint8Array): string {
  // SHA-256 常量：前 64 个素数的立方根的小数部分
  const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ])

  const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n))

  // 预处理：填充消息
  const bitLen = data.length * 8
  // 消息 + 1 字节 0x80 + 填充 + 8 字节长度，总长度对齐到 64 字节
  const padLen = (((data.length + 9 + 63) >>> 6) << 6)
  const buf = new Uint8Array(padLen)
  buf.set(data)
  buf[data.length] = 0x80
  // 写入 64 位大端长度（仅低 32 位，高 32 位在 JS 中始终为 0）
  const view = new DataView(buf.buffer)
  view.setUint32(padLen - 4, bitLen, false)

  // 初始哈希值
  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a
  let h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19

  const w = new Uint32Array(64)

  for (let offset = 0; offset < padLen; offset += 64) {
    for (let i = 0; i < 16; i++) {
      w[i] = view.getUint32(offset + i * 4, false)
    }
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3)
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10)
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0
    }

    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7

    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)
      const ch = (e & f) ^ (~e & g)
      const temp1 = (h + S1 + ch + K[i] + w[i]) | 0
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)
      const maj = (a & b) ^ (a & c) ^ (b & c)
      const temp2 = (S0 + maj) | 0

      h = g; g = f; f = e; e = (d + temp1) | 0
      d = c; c = b; b = a; a = (temp1 + temp2) | 0
    }

    h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0
    h4 = (h4 + e) | 0; h5 = (h5 + f) | 0; h6 = (h6 + g) | 0; h7 = (h7 + h) | 0
  }

  return [h0, h1, h2, h3, h4, h5, h6, h7]
    .map(v => (v >>> 0).toString(16).padStart(8, '0'))
    .join('')
}
