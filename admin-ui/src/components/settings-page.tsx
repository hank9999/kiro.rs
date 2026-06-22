import { useEffect, useState } from 'react'
import { ArrowLeft, LogOut, Save, Server } from 'lucide-react'
import { toast } from 'sonner'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { useCacheSimulation, useSetCacheSimulation } from '@/hooks/use-credentials'
import { storage } from '@/lib/storage'
import { extractErrorMessage } from '@/lib/utils'

interface SettingsPageProps {
  onBack: () => void
  onLogout: () => void
}

export function SettingsPage({ onBack, onLogout }: SettingsPageProps) {
  const queryClient = useQueryClient()
  const { data, isLoading, error } = useCacheSimulation()
  const { mutate: save, isPending } = useSetCacheSimulation()
  const [enabled, setEnabled] = useState(false)
  const [hitProbability, setHitProbability] = useState(0)
  const [minCacheRatio, setMinCacheRatio] = useState(80)
  const [maxCacheRatio, setMaxCacheRatio] = useState(90)
  const [minimumInputTokens, setMinimumInputTokens] = useState(1024)
  const [minimumUncachedTokens, setMinimumUncachedTokens] = useState(64)

  useEffect(() => {
    if (!data) return
    setEnabled(data.enabled)
    setHitProbability(data.hitProbability)
    setMinCacheRatio(data.minCacheRatio)
    setMaxCacheRatio(data.maxCacheRatio)
    setMinimumInputTokens(data.minimumInputTokens)
    setMinimumUncachedTokens(data.minimumUncachedTokens)
  }, [data])

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  const handleSave = () => {
    const normalizedProbability = Math.min(
      100,
      Math.max(0, Math.trunc(hitProbability || 0))
    )
    const normalizedMinRatio = Math.min(
      100,
      Math.max(0, Math.trunc(minCacheRatio || 0))
    )
    const normalizedMaxRatio = Math.min(
      100,
      Math.max(0, Math.trunc(maxCacheRatio || 0))
    )
    const normalizedMinimumInput = Math.max(
      0,
      Math.trunc(minimumInputTokens || 0)
    )
    const normalizedMinimumUncached = Math.max(
      0,
      Math.trunc(minimumUncachedTokens || 0)
    )

    if (normalizedMinRatio > normalizedMaxRatio) {
      toast.error('最低缓存比例不能高于最高缓存比例')
      return
    }

    save(
      {
        enabled,
        hitProbability: normalizedProbability,
        minCacheRatio: normalizedMinRatio,
        maxCacheRatio: normalizedMaxRatio,
        minimumInputTokens: normalizedMinimumInput,
        minimumUncachedTokens: normalizedMinimumUncached,
      },
      {
        onSuccess: () => {
          setHitProbability(normalizedProbability)
          setMinCacheRatio(normalizedMinRatio)
          setMaxCacheRatio(normalizedMaxRatio)
          setMinimumInputTokens(normalizedMinimumInput)
          setMinimumUncachedTokens(normalizedMinimumUncached)
          toast.success('缓存模拟设置已保存')
        },
        onError: (saveError) => {
          toast.error(`保存失败: ${extractErrorMessage(saveError)}`)
        },
      }
    )
  }

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-50 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
        <div className="container flex h-14 items-center justify-between px-4 md:px-8">
          <div className="flex items-center gap-3">
            <Button variant="ghost" size="icon" onClick={onBack} title="返回凭据管理">
              <ArrowLeft className="h-5 w-5" />
            </Button>
            <div className="flex items-center gap-2">
              <Server className="h-5 w-5" />
              <span className="font-semibold">Kiro Admin</span>
            </div>
          </div>
          <Button variant="ghost" size="icon" onClick={handleLogout} title="退出登录">
            <LogOut className="h-5 w-5" />
          </Button>
        </div>
      </header>

      <main className="container mx-auto max-w-3xl px-4 py-6 md:px-8">
        <div className="mb-6">
          <h1 className="text-2xl font-semibold">设置</h1>
          <p className="mt-1 text-sm text-muted-foreground">响应与计费字段控制</p>
        </div>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">缓存返回模拟</CardTitle>
          </CardHeader>
          <CardContent className="space-y-6">
            {isLoading ? (
              <div className="py-8 text-center text-sm text-muted-foreground">加载中...</div>
            ) : error ? (
              <div className="py-8 text-center text-sm text-destructive">
                {extractErrorMessage(error)}
              </div>
            ) : (
              <>
                <div className="flex items-center justify-between gap-4">
                  <div>
                    <div className="font-medium">启用缓存模拟</div>
                    <div className="mt-1 text-sm text-muted-foreground">
                      按命中概率和比例区间返回缓存读取 Token
                    </div>
                  </div>
                  <Switch checked={enabled} onCheckedChange={setEnabled} />
                </div>

                <div className="grid gap-5 sm:grid-cols-2">
                  <label className="space-y-2">
                    <span className="text-sm font-medium">命中概率（%）</span>
                    <Input
                      type="number"
                      min={0}
                      max={100}
                      step={1}
                      value={hitProbability}
                      onChange={(event) =>
                        setHitProbability(Number(event.target.value))
                      }
                      disabled={!enabled}
                    />
                  </label>

                  <label className="space-y-2">
                    <span className="text-sm font-medium">最低缓存比例（%）</span>
                    <Input
                      type="number"
                      min={0}
                      max={100}
                      step={1}
                      value={minCacheRatio}
                      onChange={(event) =>
                        setMinCacheRatio(Number(event.target.value))
                      }
                      disabled={!enabled}
                    />
                  </label>

                  <label className="space-y-2">
                    <span className="text-sm font-medium">最高缓存比例（%）</span>
                    <Input
                      type="number"
                      min={0}
                      max={100}
                      step={1}
                      value={maxCacheRatio}
                      onChange={(event) =>
                        setMaxCacheRatio(Number(event.target.value))
                      }
                      disabled={!enabled}
                    />
                  </label>

                  <label className="space-y-2">
                    <span className="text-sm font-medium">最低输入 Token</span>
                    <Input
                      type="number"
                      min={0}
                      step={1}
                      value={minimumInputTokens}
                      onChange={(event) =>
                        setMinimumInputTokens(Number(event.target.value))
                      }
                      disabled={!enabled}
                    />
                  </label>

                  <label className="space-y-2 sm:col-span-2">
                    <span className="text-sm font-medium">最低非缓存 Token</span>
                    <Input
                      type="number"
                      min={0}
                      step={1}
                      value={minimumUncachedTokens}
                      onChange={(event) =>
                        setMinimumUncachedTokens(Number(event.target.value))
                      }
                      disabled={!enabled}
                    />
                  </label>
                </div>

                <div className="flex justify-end border-t pt-5">
                  <Button onClick={handleSave} disabled={isPending}>
                    <Save className="mr-2 h-4 w-4" />
                    {isPending ? '保存中...' : '保存设置'}
                  </Button>
                </div>
              </>
            )}
          </CardContent>
        </Card>
      </main>
    </div>
  )
}
