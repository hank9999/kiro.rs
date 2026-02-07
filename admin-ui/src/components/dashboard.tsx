import { useState } from 'react'
import { RefreshCw, LogOut, Moon, Sun, Server, Plus, Trash2 } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { storage } from '@/lib/storage'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { CredentialCard } from '@/components/credential-card'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { SummaryModelSetting } from '@/components/summary-model-setting'
import { useCredentials, useResetAllStats, useCredentialAccountInfo } from '@/hooks/use-credentials'
import { formatExpiry, formatTokensPair } from '@/lib/format'

interface DashboardProps {
  onLogout: () => void
}

export function Dashboard({ onLogout }: DashboardProps) {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [darkMode, setDarkMode] = useState(() => {
    if (typeof window !== 'undefined') {
      return document.documentElement.classList.contains('dark')
    }
    return false
  })

  const queryClient = useQueryClient()
  const { data, isLoading, error, refetch } = useCredentials()
  const resetAllStats = useResetAllStats()

  const toggleDarkMode = () => {
    setDarkMode(!darkMode)
    document.documentElement.classList.toggle('dark')
  }

  const handleViewBalance = (id: number) => {
    setSelectedCredentialId(id)
    setBalanceDialogOpen(true)
  }

  const handleRefresh = () => {
    refetch()
    toast.success('已刷新凭据列表')
  }

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  const activeCredential =
    data?.credentials.find((c) => c.isCurrent) ||
    data?.credentials.find((c) => c.id === data?.currentId)

  const accountInfoQuery = useCredentialAccountInfo(
    activeCredential?.id ?? null,
    !!activeCredential
  )

  const formatCredits = (v: number | null | undefined) => {
    if (v === null || v === undefined) return '-'
    if (!Number.isFinite(v)) return String(v)
    return v.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })
  }

  const formatUsageLine = (
    label: string,
    current: number | null | undefined,
    limit: number | null | undefined,
    unit?: string | null
  ) => {
    const v = `${formatCredits(current)} / ${formatCredits(limit)}`
    return `${label}: ${v}${unit ? ' ' + unit : ''}`
  }

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-primary mx-auto mb-4"></div>
          <p className="text-muted-foreground">加载中...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6 text-center">
            <div className="text-red-500 mb-4">加载失败</div>
            <p className="text-muted-foreground mb-4">{(error as Error).message}</p>
            <div className="space-x-2">
              <Button onClick={() => refetch()}>重试</Button>
              <Button variant="outline" onClick={handleLogout}>重新登录</Button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="min-h-screen bg-background">
      {/* 顶部导航 */}
      <header className="sticky top-0 z-50 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
        <div className="container mx-auto flex h-14 items-center justify-between px-4 md:px-8">
          <div className="flex items-center gap-2">
            <Server className="h-5 w-5" />
            <span className="font-semibold">Kiro Admin</span>
          </div>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="icon" onClick={toggleDarkMode}>
              {darkMode ? <Sun className="h-5 w-5" /> : <Moon className="h-5 w-5" />}
            </Button>
            <Button variant="ghost" size="icon" onClick={handleRefresh}>
              <RefreshCw className="h-5 w-5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => {
                const ok = window.confirm('确定清空全部统计吗？此操作不可恢复。')
                if (!ok) return
                resetAllStats.mutate(undefined, {
                  onSuccess: (res) => toast.success(res.message),
                  onError: (err) => toast.error('操作失败: ' + (err as Error).message),
                })
              }}
              disabled={resetAllStats.isPending}
              title="清空全部统计"
            >
              <Trash2 className="h-5 w-5" />
            </Button>
            <Button variant="ghost" size="icon" onClick={handleLogout}>
              <LogOut className="h-5 w-5" />
            </Button>
          </div>
        </div>
      </header>

      {/* 主内容 */}
      <main className="container mx-auto px-4 md:px-8 py-6">
        {/* 统计卡片 */}
        <div className="grid gap-4 md:grid-cols-4 mb-6">
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">
                凭据总数
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold">{data?.total || 0}</div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">
                可用凭据
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold text-green-600">{data?.available || 0}</div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">
                当前活跃
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold flex items-center gap-2">
                #{data?.currentId || '-'}
                <Badge variant="success">活跃</Badge>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">
                主账户信息
              </CardTitle>
            </CardHeader>
            <CardContent>
              {activeCredential ? (
                <div className="space-y-1">
                  <div className="font-semibold">凭据 #{activeCredential.id}</div>
                  <div className="text-sm text-muted-foreground">
                    认证：{activeCredential.authMethod || '未知'}
                    {activeCredential.hasProfileArn ? ' · 有 Profile ARN' : ''}
                  </div>
                  <div className="text-sm text-muted-foreground">
                    邮箱：{accountInfoQuery.data?.account.email || activeCredential.accountEmail || '-'}
                  </div>
                  <div className="text-sm text-muted-foreground">
                    套餐：{accountInfoQuery.data?.account.subscriptionTitle || accountInfoQuery.data?.account.subscriptionType || '-'}
                  </div>

                  {accountInfoQuery.error ? (
                    <div className="text-xs text-red-500">
                      账号信息拉取失败：{(accountInfoQuery.error as Error).message}
                    </div>
                  ) : null}

                  <div className="text-sm">
                    用量：
                    {(() => {
                      const account = accountInfoQuery.data?.account
                      if (!account) return '-'

                      const usage = account.usage
                      const resources = account.resources || []
                      const hasCreditResource = resources.some((r) =>
                        (r.resourceType || '').toUpperCase().includes('CREDIT')
                      )

                      const unit = usage.resourceDetail?.unit || usage.resourceDetail?.currency || ''
                      const bonuses = usage.bonuses || []

                      const hasCreditsMeaningful =
                        hasCreditResource &&
                        (usage.limit > 0 ||
                          usage.current > 0 ||
                          usage.baseLimit > 0 ||
                          usage.baseCurrent > 0 ||
                          usage.freeTrialLimit > 0 ||
                          usage.freeTrialCurrent > 0 ||
                          bonuses.length > 0)

                      // 结构化渲染，避免依赖前导空格缩进
                      type Line = { text: string; muted?: boolean; indent?: boolean }
                      const lines: Line[] = []

                      if (hasCreditsMeaningful) {
                        lines.push({ text: formatUsageLine('总计', usage.current, usage.limit, unit) })

                        if (usage.baseLimit > 0 || usage.baseCurrent > 0) {
                          lines.push({ text: formatUsageLine('基础', usage.baseCurrent, usage.baseLimit, unit) })
                        }

                        if (usage.freeTrialLimit > 0 || usage.freeTrialCurrent > 0) {
                          const expiry = usage.freeTrialExpiry ? `，到期 ${usage.freeTrialExpiry}` : ''
                          lines.push({
                            text: `${formatUsageLine('试用', usage.freeTrialCurrent, usage.freeTrialLimit, unit)}${expiry}`,
                          })
                        }

                        if (bonuses.length > 0) {
                          const bCurrent = bonuses.reduce((acc, b) => acc + (b.current || 0), 0)
                          const bLimit = bonuses.reduce((acc, b) => acc + (b.limit || 0), 0)
                          lines.push({ text: formatUsageLine('赠送', bCurrent, bLimit, unit) })
                        }
                      } else {
                        // 没有 Credits 或 Credits 不适用：展示最主要的一个资源作为“总计”
                        const preferred = resources[0]
                        if (preferred) {
                          const label = preferred.displayName || preferred.resourceType || 'Usage'
                          const u = preferred.unit || preferred.currency || ''
                          lines.push({ text: formatUsageLine(label, preferred.current, preferred.limit, u) })
                        } else {
                          return '-'
                        }
                      }

                      // 其它资源（例如 MONTHLY_REQUEST_COUNT 等）
                      const others = resources.filter((r) => {
                        const t = (r.resourceType || '').toUpperCase()
                        return hasCreditsMeaningful ? !t.includes('CREDIT') : true
                      })

                      if (others.length > 0) {
                        const top = others.slice(0, 6)
                        lines.push({ text: '其它资源：', muted: true })
                        for (const r of top) {
                          const label = r.displayName || r.resourceType || 'Usage'
                          const u = r.unit || r.currency || ''
                          lines.push({ text: formatUsageLine(label, r.current, r.limit, u), indent: true })
                        }
                        if (others.length > top.length) {
                          lines.push({ text: `...以及 ${others.length - top.length} 项`, indent: true, muted: true })
                        }
                      }

                      return (
                        <div className="space-y-0.5">
                          {lines.map((l, idx) => (
                            <div
                              key={idx}
                              className={[l.muted ? 'text-muted-foreground' : '', l.indent ? 'pl-4' : '']
                                .filter(Boolean)
                                .join(' ')}
                            >
                              {l.text}
                            </div>
                          ))}
                        </div>
                      )
                    })()}
                  </div>
                  <div className="text-sm text-muted-foreground">
                    Token 有效期：{formatExpiry(activeCredential.expiresAt)}
                  </div>
                  <div className="text-sm">
                    Tokens：{formatTokensPair(activeCredential.inputTokensTotal, activeCredential.outputTokensTotal)}
                  </div>
                </div>
              ) : (
                <div className="text-sm text-muted-foreground">暂无</div>
              )}
            </CardContent>
          </Card>
        </div>

        {/* 设置区域 */}
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3 mb-6">
          <SummaryModelSetting />
        </div>

        {/* 凭据列表 */}
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <h2 className="text-xl font-semibold">凭据管理</h2>
            <Button onClick={() => setAddDialogOpen(true)} size="sm">
              <Plus className="h-4 w-4 mr-2" />
              添加凭据
            </Button>
          </div>
          {data?.credentials.length === 0 ? (
            <Card>
              <CardContent className="py-8 text-center text-muted-foreground">
                暂无凭据
              </CardContent>
            </Card>
          ) : (
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {data?.credentials.map((credential) => (
                <CredentialCard
                  key={credential.id}
                  credential={credential}
                  onViewBalance={handleViewBalance}
                />
              ))}
            </div>
          )}
        </div>
      </main>

      {/* 余额对话框 */}
      <BalanceDialog
        credentialId={selectedCredentialId}
        open={balanceDialogOpen}
        onOpenChange={setBalanceDialogOpen}
      />

      {/* 添加凭据对话框 */}
      <AddCredentialDialog
        open={addDialogOpen}
        onOpenChange={setAddDialogOpen}
      />
    </div>
  )
}
