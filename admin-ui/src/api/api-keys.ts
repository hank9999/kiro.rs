import axios from "axios";
import { storage } from "@/lib/storage";
import type {
  ApiKeysListResponse,
  ApiKeyInfo,
  AddApiKeyRequest,
  GenerateApiKeyRequest,
  GenerateApiKeyResponse,
  UpdateApiKeyRequest,
  SuccessResponse,
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

// 获取所有 API Keys
export async function getApiKeys(): Promise<ApiKeysListResponse> {
  const { data } = await api.get<ApiKeysListResponse>("/api-keys");
  return data;
}

// 添加新的 API Key
export async function addApiKey(
  request: AddApiKeyRequest,
): Promise<ApiKeyInfo> {
  const { data } = await api.post<ApiKeyInfo>("/api-keys", request);
  return data;
}

// 生成随机 API Key
export async function generateApiKey(
  request: GenerateApiKeyRequest,
): Promise<GenerateApiKeyResponse> {
  const { data } = await api.post<GenerateApiKeyResponse>(
    "/api-keys/generate",
    request,
  );
  return data;
}

// 更新 API Key
export async function updateApiKey(
  id: string,
  request: UpdateApiKeyRequest,
): Promise<SuccessResponse> {
  const { data } = await api.put<SuccessResponse>(`/api-keys/${id}`, request);
  return data;
}

// 删除 API Key
export async function deleteApiKey(id: string): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/api-keys/${id}`);
  return data;
}
