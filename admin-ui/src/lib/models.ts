export const MODEL_OPTIONS = [
  { id: 'claude-sonnet-4.5', label: 'Sonnet 4.5' },
  { id: 'claude-sonnet-4', label: 'Sonnet 4' },
  { id: 'claude-haiku-4.5', label: 'Haiku 4.5' },
  { id: 'claude-opus-4.5', label: 'Opus 4.5' },
] as const

export type SupportedModelId = (typeof MODEL_OPTIONS)[number]['id']

export const ALL_MODEL_IDS: SupportedModelId[] = MODEL_OPTIONS.map((m) => m.id)

// 后端返回 null/undefined 代表"未配置"，UI 解释为"默认全开"
export function normalizeEnabledModels(
  enabledModels: string[] | null | undefined
): SupportedModelId[] {
  if (enabledModels === null || enabledModels === undefined) {
    return [...ALL_MODEL_IDS]
  }
  const set = new Set(enabledModels)
  return MODEL_OPTIONS.filter((m) => set.has(m.id)).map((m) => m.id)
}
