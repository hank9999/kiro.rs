import axios from "axios";
import { storage } from "@/lib/storage";
import type {
  ProxyPoolConfig,
  ProxyPoolStatus,
  ProxyTestResponse,
  SuccessResponse,
  TestProxyPoolRequest,
  UpdateCredentialProxyRequest,
} from "@/types/api";

const api = axios.create({
  baseURL: "/api/admin",
  headers: {
    "Content-Type": "application/json",
  },
});

api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey();
  if (apiKey) {
    config.headers["x-api-key"] = apiKey;
  }
  return config;
});

// 获取当前代理池配置与状态
export async function getProxyPool(): Promise<ProxyPoolStatus> {
  const { data } = await api.get<ProxyPoolStatus>("/proxy-pool");
  return data;
}

// 更新代理池配置（持久化 + 热更新）
export async function updateProxyPool(
  payload: ProxyPoolConfig,
): Promise<ProxyPoolStatus> {
  const { data } = await api.put<ProxyPoolStatus>("/proxy-pool", payload);
  return data;
}

// 测试代理池连通性
export async function testProxyPool(
  payload: TestProxyPoolRequest = {},
): Promise<ProxyTestResponse> {
  const { data } = await api.post<ProxyTestResponse>(
    "/proxy-pool/test",
    payload,
  );
  return data;
}

// 更新单个凭据的代理配置
export async function updateCredentialProxy(
  id: number,
  payload: UpdateCredentialProxyRequest,
): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(
    `/credentials/${id}/proxy`,
    payload,
  );
  return data;
}
