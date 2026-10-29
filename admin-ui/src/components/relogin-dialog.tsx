import { useState, useEffect, useRef } from 'react'
import { toast } from 'sonner'
import { ExternalLink, Copy, Loader2, CheckCircle } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  startSocialRelogin,
  pollSocialRelogin,
  completeSocialRelogin,
  startIdcRelogin,
  pollIdcRelogin,
} from '@/api/credentials'
import {
  useUpdateRefreshToken,
  useSetDisabled,
  useResetFailure,
} from '@/hooks/use-credentials'
import type { CredentialStatusItem, StartSocialLoginResponse, StartIdcLoginResponse } from '@/types/api'
import { extractErrorMessage } from '@/lib/utils'
import { useQueryClient } from '@tanstack/react-query'

interface ReloginDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  credential: CredentialStatusItem
}

type Method = 'social' | 'idc' | 'manual'
type Step = 'select' | 'form' | 'waiting' | 'manual-updating' | 'done'

const POLL_INTERVAL_MS = 2000

const isRemoteAccess = () =>
  window.location.hostname !== 'localhost' && window.location.hostname !== '127.0.0.1'

function parseCallbackUrl(rawUrl: string): { code: string; state: string; loginOption: string; path: string } | null {
  try {
    const url = new URL(rawUrl.trim())
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    if (!code || !state) return null
    return {
      code,
      state,
      loginOption: url.searchParams.get('login_option') ?? '',
      path: url.pathname,
    }
  } catch {
    return null
  }
}

interface ParsedTokenData {
  refreshToken: string
  email?: string
}

function parseTokenInput(input: string): ParsedTokenData {
  const trimmed = input.trim()
  if (!trimmed) return { refreshToken: '' }

  try {
    const parsed = JSON.parse(trimmed)
    const extractFromObj = (obj: Record<string, unknown>): ParsedTokenData | null => {
      const rt = typeof obj.refreshToken === 'string' ? obj.refreshToken.trim() : ''
      if (!rt) return null
      const email = typeof obj.email === 'string' ? obj.email.trim() : undefined
      return { refreshToken: rt, email: email || undefined }
    }

    const direct = extractFromObj(parsed as Record<string, unknown>)
    if (direct) return direct

    if (parsed.credentials) {
      const nested = extractFromObj(parsed.credentials as Record<string, unknown>)
      if (nested) {
        const outerEmail = typeof (parsed as Record<string, unknown>).email === 'string'
          ? ((parsed as Record<string, unknown>).email as string).trim() || undefined
          : undefined
        return { ...nested, email: nested.email ?? outerEmail }
      }
    }

    if (Array.isArray(parsed) && parsed.length > 0) {
      const first = parsed[0] as Record<string, unknown>
      const fromFirst = extractFromObj(first)
      if (fromFirst) return fromFirst
    }

    return { refreshToken: '' }
  } catch {
    return { refreshToken: trimmed }
  }
}

export function ReloginDialog({ open, onOpenChange, credential }: ReloginDialogProps) {
  const [method, setMethod] = useState<Method>('social')
  const [step, setStep] = useState<Step>('select')

  // Social/IdC 表单字段
  const [isStarting, setIsStarting] = useState(false)
  const [isCompleting, setIsCompleting] = useState(false)
  const [callbackUrl, setCallbackUrl] = useState('')
  const [socialSession, setSocialSession] = useState<StartSocialLoginResponse | null>(null)
  const [idcSession, setIdcSession] = useState<StartIdcLoginResponse | null>(null)
  // IdC 表单
  const [idcRegion, setIdcRegion] = useState('us-east-1')
  const [idcStartUrl, setIdcStartUrl] = useState('')

  // Manual 字段
  const [manualInput, setManualInput] = useState('')
  const [manualLog, setManualLog] = useState<string[]>([])

  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const isRemote = isRemoteAccess()

  const queryClient = useQueryClient()
  const updateRefreshToken = useUpdateRefreshToken()
  const setDisabled = useSetDisabled()
  const resetFailure = useResetFailure()

  useEffect(() => {
    return () => {
      if (pollTimerRef.current) clearTimeout(pollTimerRef.current)
    }
  }, [])

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['credentials'] })

  const handleClose = () => {
    if (pollTimerRef.current) clearTimeout(pollTimerRef.current)
    setStep('select')
    setSocialSession(null)
    setIdcSession(null)
    setIsStarting(false)
    setIsCompleting(false)
    setCallbackUrl('')
    setManualInput('')
    setManualLog([])
    onOpenChange(false)
  }

  // ─── Social ───────────────────────────────────────────────────────────────

  const handleStartSocial = async () => {
    setIsStarting(true)
    // 必须在 await 之前同步打开窗口，否则浏览器弹窗拦截会导致跳转当前页
    const loginWindow = window.open('about:blank', '_blank')
    try {
      const resp = await startSocialRelogin(credential.id, {})
      setSocialSession(resp)
      setStep('waiting')
      if (loginWindow) {
        loginWindow.location.href = resp.portalUrl
      } else {
        window.open(resp.portalUrl, '_blank')
      }
      if (!isRemote) scheduleSocialPoll(resp.sessionId)
    } catch (e) {
      loginWindow?.close()
      toast.error('发起登录失败：' + extractErrorMessage(e))
    } finally {
      setIsStarting(false)
    }
  }

  const scheduleSocialPoll = (sessionId: string) => {
    pollTimerRef.current = setTimeout(async () => {
      try {
        const result = await pollSocialRelogin(credential.id, sessionId)
        if (result.status === 'pending') {
          scheduleSocialPoll(sessionId)
        } else if (result.status === 'success') {
          setStep('done')
          invalidate()
          toast.success(`凭据 #${result.credentialId} Token 已更新并启用`)
        } else {
          toast.error('会话已过期，请重新发起登录')
          setStep('form')
          setSocialSession(null)
        }
      } catch (e) {
        toast.error('轮询失败：' + extractErrorMessage(e))
        scheduleSocialPoll(sessionId)
      }
    }, POLL_INTERVAL_MS)
  }

  const handleCompleteSocialManually = async () => {
    if (!socialSession) return
    const parsed = parseCallbackUrl(callbackUrl)
    if (!parsed) {
      toast.error('URL 格式无效，请复制完整的地址栏 URL')
      return
    }
    setIsCompleting(true)
    try {
      const result = await completeSocialRelogin(credential.id, socialSession.sessionId, {
        code: parsed.code,
        state: parsed.state,
        loginOption: parsed.loginOption || undefined,
        path: parsed.path,
      })
      if (result.status === 'success') {
        setStep('done')
        invalidate()
        toast.success(`凭据 #${result.credentialId} Token 已更新并启用`)
      } else {
        toast.error('会话已过期，请重新发起登录')
        setStep('form')
        setSocialSession(null)
      }
    } catch (e) {
      toast.error('完成登录失败：' + extractErrorMessage(e))
    } finally {
      setIsCompleting(false)
    }
  }

  // ─── IdC ──────────────────────────────────────────────────────────────────

  const handleStartIdc = async () => {
    if (!idcRegion.trim()) {
      toast.error('请填写 AWS Region')
      return
    }
    setIsStarting(true)
    try {
      const resp = await startIdcRelogin(credential.id, {
        region: idcRegion.trim(),
        startUrl: idcStartUrl.trim() || undefined,
      })
      setIdcSession(resp)
      setStep('waiting')
      scheduleIdcPoll(resp.sessionId, resp.pollInterval)
    } catch (e) {
      toast.error('发起登录失败：' + extractErrorMessage(e))
    } finally {
      setIsStarting(false)
    }
  }

  const scheduleIdcPoll = (sessionId: string, interval: number) => {
    pollTimerRef.current = setTimeout(async () => {
      try {
        const result = await pollIdcRelogin(credential.id, sessionId)
        if (result.status === 'pending') {
          scheduleIdcPoll(sessionId, interval)
        } else if (result.status === 'success') {
          setStep('done')
          invalidate()
          toast.success(`凭据 #${result.credentialId} Token 已更新并启用`)
        } else {
          toast.error('授权已过期，请重新发起登录')
          setStep('form')
          setIdcSession(null)
        }
      } catch (e) {
        toast.error('轮询失败：' + extractErrorMessage(e))
        scheduleIdcPoll(sessionId, interval)
      }
    }, interval * 1000)
  }

  const copyIdcCode = () => {
    if (!idcSession) return
    navigator.clipboard.writeText(idcSession.userCode)
    toast.success('验证码已复制')
  }

  // ─── Manual ───────────────────────────────────────────────────────────────

  const parsed = parseTokenInput(manualInput)
  const extractedToken = parsed.refreshToken
  const isManualValid = extractedToken.length >= 100 && !extractedToken.includes('...')
  const isManualUpdating = step === 'manual-updating'

  const addLog = (msg: string) => setManualLog(prev => [...prev, msg])

  const handleManualSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!isManualValid) {
      toast.error('refreshToken 无效或已被截断')
      return
    }
    setStep('manual-updating')
    setManualLog([])

    try {
      if (!credential.disabled) {
        addLog('正在临时禁用凭据…')
        await new Promise<void>((resolve, reject) => {
          setDisabled.mutate({ id: credential.id, disabled: true }, { onSuccess: () => resolve(), onError: reject })
        })
        addLog('✓ 已临时禁用')
      }

      addLog('正在更新 refreshToken…')
      await new Promise<void>((resolve, reject) => {
        updateRefreshToken.mutate(
          { id: credential.id, req: { refreshToken: extractedToken } },
          { onSuccess: () => resolve(), onError: reject }
        )
      })
      addLog('✓ refreshToken 已更新')

      addLog('正在重置失败计数并启用…')
      await new Promise<void>((resolve, reject) => {
        resetFailure.mutate(credential.id, { onSuccess: () => resolve(), onError: reject })
      })
      addLog('✓ 已重置并启用')

      setStep('done')
      invalidate()
      toast.success(`凭据 #${credential.id} 重新导入完成，已自动启用`)
    } catch (error) {
      addLog(`✗ 失败: ${extractErrorMessage(error)}`)
      setStep('select')
      toast.error(`操作失败: ${extractErrorMessage(error)}`)
    }
  }

  // ─── Render ───────────────────────────────────────────────────────────────

  const displayName = credential.email || `凭据 #${credential.id}`
  const authMethod = credential.authMethod

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>重新登录 — {displayName}</DialogTitle>
          <DialogDescription>
            选择登录方式，完成后将刷新该凭据的 Token 并自动重新启用。
          </DialogDescription>
        </DialogHeader>

        {/* 方式选择 */}
        {step === 'select' && (
          <div className="space-y-3 py-2">
            <p className="text-sm text-muted-foreground">选择重新登录方式：</p>
            <div className="grid gap-2">
              <button
                onClick={() => { setMethod('social'); setStep('form') }}
                className={`flex items-start gap-3 rounded-lg border p-3 text-left transition-colors hover:bg-accent ${authMethod === 'social' ? 'border-primary bg-accent/50' : ''}`}
              >
                <div>
                  <p className="text-sm font-medium">Social 登录（Google / GitHub）</p>
                  <p className="text-xs text-muted-foreground mt-0.5">通过 Kiro 网页端完成 OAuth 授权</p>
                </div>
              </button>
              <button
                onClick={() => { setMethod('idc'); setStep('form') }}
                className={`flex items-start gap-3 rounded-lg border p-3 text-left transition-colors hover:bg-accent ${authMethod === 'idc' ? 'border-primary bg-accent/50' : ''}`}
              >
                <div>
                  <p className="text-sm font-medium">AWS SSO / Builder ID（IdC）</p>
                  <p className="text-xs text-muted-foreground mt-0.5">通过 AWS Identity Center 设备授权</p>
                </div>
              </button>
              <button
                onClick={() => { setMethod('manual'); setStep('form') }}
                className="flex items-start gap-3 rounded-lg border p-3 text-left transition-colors hover:bg-accent"
              >
                <div>
                  <p className="text-sm font-medium">手动粘贴 Token</p>
                  <p className="text-xs text-muted-foreground mt-0.5">粘贴 KAM JSON 或 refreshToken 字符串</p>
                </div>
              </button>
            </div>
          </div>
        )}

        {/* Social 表单 */}
        {step === 'form' && method === 'social' && (
          <div className="py-2 space-y-3">
            <p className="text-sm text-muted-foreground">
              点击「发起登录」，浏览器将打开 Kiro 登录页。完成授权后，Token 将自动更新到本凭据。
            </p>
          </div>
        )}

        {/* Social 等待 */}
        {step === 'waiting' && method === 'social' && socialSession && (
          <div className="space-y-4 py-2">
            <div className="rounded-lg border bg-muted/50 p-4 space-y-3">
              <p className="text-sm text-muted-foreground">浏览器应已自动打开 Kiro 登录页，请完成授权。</p>
              <a
                href={socialSession.portalUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 text-sm font-medium text-primary hover:underline"
              >
                重新打开登录页
                <ExternalLink className="h-3.5 w-3.5" />
              </a>
            </div>
            {isRemote ? (
              <div className="space-y-2">
                <p className="text-sm text-amber-600 dark:text-amber-400">
                  完成登录后，从地址栏复制完整 URL 粘贴到下方：
                </p>
                <textarea
                  placeholder="http://localhost:3128/oauth/callback?code=...&state=...&login_option=google"
                  value={callbackUrl}
                  onChange={(e) => setCallbackUrl(e.target.value)}
                  disabled={isCompleting}
                  className="flex min-h-[80px] w-full rounded-md border border-input bg-background px-3 py-2 text-xs font-mono placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
                />
              </div>
            ) : (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin" />
                正在等待登录完成…
              </div>
            )}
          </div>
        )}

        {/* IdC 表单 */}
        {step === 'form' && method === 'idc' && (
          <div className="space-y-3 py-2">
            <div className="space-y-1.5">
              <label className="text-sm font-medium">AWS Region</label>
              <Input
                placeholder="us-east-1"
                value={idcRegion}
                onChange={(e) => setIdcRegion(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-sm font-medium">
                SSO Start URL
                <span className="ml-1 text-xs text-muted-foreground">（留空使用 AWS Builder ID）</span>
              </label>
              <Input
                placeholder="https://view.awsapps.com/start"
                value={idcStartUrl}
                onChange={(e) => setIdcStartUrl(e.target.value)}
              />
            </div>
          </div>
        )}

        {/* IdC 等待 */}
        {step === 'waiting' && method === 'idc' && idcSession && (
          <div className="space-y-4 py-2">
            <div className="rounded-lg border bg-muted/50 p-4 text-center space-y-3">
              <p className="text-sm text-muted-foreground">在浏览器中访问以下地址并输入验证码</p>
              <a
                href={idcSession.verificationUriComplete ?? idcSession.verificationUri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 text-sm font-medium text-primary hover:underline"
              >
                {idcSession.verificationUri}
                <ExternalLink className="h-3.5 w-3.5" />
              </a>
              <div className="flex items-center justify-center gap-2">
                <span className="font-mono text-2xl font-bold tracking-widest">{idcSession.userCode}</span>
                <Button variant="ghost" size="icon" className="h-7 w-7" onClick={copyIdcCode}>
                  <Copy className="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              正在等待授权，请在浏览器中完成登录…
            </div>
          </div>
        )}

        {/* Manual 表单 */}
        {(step === 'form' || step === 'manual-updating') && method === 'manual' && (
          <form onSubmit={handleManualSubmit}>
            <div className="space-y-3 py-2">
              <label className="text-sm font-medium">
                粘贴 KAM 导出 JSON 或 refreshToken 字符串
              </label>
              <textarea
                placeholder={'支持以下格式：\n\n1. 直接粘贴 refreshToken 字符串\n\n2. KAM 导出 JSON：\n{\n  "email": "...",\n  "refreshToken": "aor..."\n}'}
                value={manualInput}
                onChange={(e) => setManualInput(e.target.value)}
                disabled={isManualUpdating}
                className="flex min-h-[140px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:opacity-50 font-mono"
              />
              {manualInput.trim() && step === 'form' && (
                <div className={`text-sm rounded-md p-3 ${isManualValid ? 'bg-green-50 dark:bg-green-950 text-green-700 dark:text-green-300' : 'bg-red-50 dark:bg-red-950 text-red-700 dark:text-red-300'}`}>
                  {isManualValid ? (
                    <>
                      已识别 refreshToken（{extractedToken.length} 字符）：
                      <span className="font-mono text-xs block mt-1 opacity-75">
                        {extractedToken.slice(0, 20)}...{extractedToken.slice(-10)}
                      </span>
                    </>
                  ) : (
                    extractedToken.length > 0
                      ? `Token 无效：长度 ${extractedToken.length} 字符（需要 ≥100 字符）`
                      : '无法识别 refreshToken，请检查格式'
                  )}
                </div>
              )}
              {manualLog.length > 0 && (
                <div className="rounded-md border bg-muted/40 p-3 space-y-1">
                  {manualLog.map((log, i) => (
                    <div key={i} className="text-sm font-mono">{log}</div>
                  ))}
                </div>
              )}
            </div>
          </form>
        )}

        {/* 完成 */}
        {step === 'done' && (
          <div className="flex flex-col items-center gap-3 py-4">
            <CheckCircle className="h-10 w-10 text-green-500" />
            <p className="text-sm font-medium">Token 已更新，凭据已启用</p>
            <p className="text-xs text-muted-foreground">{displayName}</p>
          </div>
        )}

        <DialogFooter>
          {step === 'select' && (
            <Button variant="outline" onClick={handleClose}>取消</Button>
          )}

          {step === 'form' && method === 'social' && (
            <>
              <Button variant="outline" onClick={() => setStep('select')}>返回</Button>
              <Button onClick={handleStartSocial} disabled={isStarting}>
                {isStarting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                发起登录
              </Button>
            </>
          )}

          {step === 'waiting' && method === 'social' && (
            <>
              <Button variant="outline" onClick={handleClose} disabled={isCompleting}>取消</Button>
              {isRemote && (
                <Button
                  onClick={handleCompleteSocialManually}
                  disabled={isCompleting || !callbackUrl.trim()}
                >
                  {isCompleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                  完成登录
                </Button>
              )}
            </>
          )}

          {step === 'form' && method === 'idc' && (
            <>
              <Button variant="outline" onClick={() => setStep('select')}>返回</Button>
              <Button onClick={handleStartIdc} disabled={isStarting}>
                {isStarting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                发起登录
              </Button>
            </>
          )}

          {step === 'waiting' && method === 'idc' && (
            <Button variant="outline" onClick={handleClose}>取消</Button>
          )}

          {(step === 'form' || step === 'manual-updating') && method === 'manual' && (
            <>
              <Button variant="outline" onClick={() => setStep('select')} disabled={isManualUpdating}>返回</Button>
              <Button
                onClick={(e) => handleManualSubmit(e as unknown as React.FormEvent)}
                disabled={isManualUpdating || !isManualValid}
              >
                {isManualUpdating ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
                {isManualUpdating ? '处理中…' : '确认更新'}
              </Button>
            </>
          )}

          {step === 'done' && (
            <Button onClick={handleClose}>关闭</Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
