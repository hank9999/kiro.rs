import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  FlowListResponse,
  FlowStatsResponse,
  FlowQuery,
  SuccessResponse,
} from '@/types/api'

// 创建 axios 实例（复用与 credentials 相同的模式）
const api = axios.create({
  baseURL: '/api/admin',
  headers: {
    'Content-Type': 'application/json',
  },
})

// 请求拦截器添加 API Key
api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) {
    config.headers['x-api-key'] = apiKey
  }
  return config
})

// 获取流量记录列表
export async function getFlows(query: FlowQuery): Promise<FlowListResponse> {
  const { data } = await api.get<FlowListResponse>('/flows', { params: query })
  return data
}

// 获取流量统计
export async function getFlowStats(): Promise<FlowStatsResponse> {
  const { data } = await api.get<FlowStatsResponse>('/flows/stats')
  return data
}

// 清空流量记录
export async function clearFlows(before?: string): Promise<SuccessResponse> {
  const params = before ? { before } : undefined
  const { data } = await api.delete<SuccessResponse>('/flows', { params })
  return data
}

// 检查流量监控是否可用
export async function checkFlowMonitorAvailable(): Promise<boolean> {
  try {
    await api.get('/flows/stats')
    return true
  } catch (error) {
    if (axios.isAxiosError(error) && error.response?.status === 404) {
      return false
    }
    // Non-404 errors (auth, network, server) — assume feature exists
    return true
  }
}
