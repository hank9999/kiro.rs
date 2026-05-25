import axios from "axios";
import { storage } from "@/lib/storage";
import type {
  CredentialsStatusResponse,
  BalanceResponse,
  BatchCredentialBalanceResponse,
  SuccessResponse,
  SetDisabledRequest,
  SetPriorityRequest,
  AddCredentialRequest,
  AddCredentialResponse,
  LogsResponse,
  RequestActivityResponse,
  AvailableModelsResponse,
} from "@/types/api";

// 创建 axios 实例
const api = axios.create({
  baseURL: "/api/admin",
  headers: {
    "Content-Type": "application/json",
  },
});

// 请求拦截器添加 API Key
api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey();
  if (apiKey) {
    config.headers["x-api-key"] = apiKey;
  }
  return config;
});

// 获取所有凭据状态
export async function getCredentials(): Promise<CredentialsStatusResponse> {
  const { data } = await api.get<CredentialsStatusResponse>("/credentials");
  return data;
}

// 设置凭据禁用状态
export async function setCredentialDisabled(
  id: number,
  disabled: boolean,
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/disabled`,
    { disabled } as SetDisabledRequest,
  );
  return data;
}

// 设置凭据优先级
export async function setCredentialPriority(
  id: number,
  priority: number,
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/priority`,
    { priority } as SetPriorityRequest,
  );
  return data;
}

// 重置失败计数
export async function resetCredentialFailure(
  id: number,
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/reset`);
  return data;
}

// 强制刷新 Token
export async function forceRefreshToken(id: number): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/refresh`,
  );
  return data;
}

// 获取凭据余额
export async function getCredentialBalance(
  id: number,
): Promise<BalanceResponse> {
  const { data } = await api.get<BalanceResponse>(`/credentials/${id}/balance`);
  return data;
}

// 查询所有凭据余额，并自动启用仍有剩余额度的账号
export async function queryAllCredentialBalancesAndEnable(): Promise<BatchCredentialBalanceResponse> {
  const { data } = await api.post<BatchCredentialBalanceResponse>(
    "/credentials/query-balances-enable",
  );
  return data;
}

// 启用所有可恢复凭据
export async function enableAllCredentials(): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>("/credentials/enable-all");
  return data;
}

// 添加新凭据
export async function addCredential(
  req: AddCredentialRequest,
): Promise<AddCredentialResponse> {
  const { data } = await api.post<AddCredentialResponse>("/credentials", req);
  return data;
}

// 删除凭据
export async function deleteCredential(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/credentials/${id}`);
  return data;
}

// 负载均衡模式联合类型
export type LoadBalancingMode = "priority" | "balanced" | "round_robin";

// 获取负载均衡模式
export async function getLoadBalancingMode(): Promise<{
  mode: LoadBalancingMode;
}> {
  const { data } = await api.get<{ mode: LoadBalancingMode }>(
    "/config/load-balancing",
  );
  return data;
}

// 设置负载均衡模式
export async function setLoadBalancingMode(
  mode: LoadBalancingMode,
): Promise<{ mode: LoadBalancingMode }> {
  const { data } = await api.put<{ mode: LoadBalancingMode }>(
    "/config/load-balancing",
    { mode },
  );
  return data;
}

// 获取最近请求活动
export async function getRequestActivity(
  limit = 50,
): Promise<RequestActivityResponse> {
  const { data } = await api.get<RequestActivityResponse>("/activity", {
    params: { limit },
  });
  return data;
}

// 获取最近日志
export async function getRecentLogs(lines = 120): Promise<LogsResponse> {
  const { data } = await api.get<LogsResponse>("/logs", {
    params: { lines },
  });
  return data;
}

// 获取可用模型
export async function getAvailableModels(): Promise<AvailableModelsResponse> {
  const { data } = await api.get<AvailableModelsResponse>("/models");
  return data;
}
