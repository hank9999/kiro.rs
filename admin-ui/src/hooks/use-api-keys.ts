import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  getApiKeys,
  addApiKey,
  generateApiKey,
  updateApiKey,
  deleteApiKey,
} from "@/api/api-keys";
import type {
  AddApiKeyRequest,
  GenerateApiKeyRequest,
  UpdateApiKeyRequest,
} from "@/types/api";

// 查询 API Keys 列表
export function useApiKeys() {
  return useQuery({
    queryKey: ["api-keys"],
    queryFn: getApiKeys,
    refetchInterval: 30000, // 每 30 秒刷新一次
  });
}

// 添加 API Key
export function useAddApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: AddApiKeyRequest) => addApiKey(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
}

// 生成随机 API Key
export function useGenerateApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: GenerateApiKeyRequest) => generateApiKey(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
}

// 更新 API Key
export function useUpdateApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, request }: { id: string; request: UpdateApiKeyRequest }) =>
      updateApiKey(id, request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
}

// 删除 API Key
export function useDeleteApiKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => deleteApiKey(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
}
