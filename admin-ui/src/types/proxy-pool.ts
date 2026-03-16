// 代理池条目
export interface ProxyEntry {
  id: number
  url: string
  username?: string
  password?: string
  label: string
  disabled: boolean
  assignedTo?: number | null
}

// 代理池列表响应
export interface ProxyPoolListResponse {
  total: number
  available: number
  proxies: ProxyEntry[]
}

// 添加代理请求
export interface AddProxyRequest {
  url: string
  username?: string
  password?: string
  label?: string
}

// 批量添加代理请求
export interface BatchAddProxyRequest {
  lines: string[]
}

// 编辑代理请求
export interface UpdateProxyRequest {
  url?: string
  username?: string | null
  password?: string | null
  label?: string
  disabled?: boolean
}

// 代理测试响应
export interface ProxyTestResponse {
  success: boolean
  message: string
  latencyMs?: number
}
