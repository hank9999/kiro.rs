import { useState, useEffect, useRef } from 'react'
import { toast } from 'sonner'
import { ExternalLink, CheckCircle, Loader2 } from 'lucide-react'
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
import { startSocialLogin, pollSocialLogin, completeSocialLogin } from '@/api/credentials'
import type { StartSocialLoginResponse } from '@/types/api'
import { extractErrorMessage } from '@/lib/utils'

interface SocialLoginDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSuccess: () => void
}

type Step = 'form' | 'waiting' | 'done'

const POLL_INTERVAL_MS = 2000

// 检查是否为远程访问（非本机）
const isRemoteAccess = () =>
  window.location.hostname !== 'localhost' && window.location.hostname !== '127.0.0.1'

// 从回调 URL 字符串中提取 OAuth 参数
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

export function SocialLoginDialog({ open, onOpenChange, onSuccess }: SocialLoginDialogProps) {
  const [step, setStep] = useState<Step>('form')
  const [priority, setPriority] = useState('0')
  const [email, setEmail] = useState('')
  const [isStarting, setIsStarting] = useState(false)
  const [isCompleting, setIsCompleting] = useState(false)
  const [session, setSession] = useState<StartSocialLoginResponse | null>(null)
  const [credentialId, setCredentialId] = useState<number | null>(null)
  const [callbackUrl, setCallbackUrl] = useState('')
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const isRemote = isRemoteAccess()

  useEffect(() => {
    return () => {
      if (pollTimerRef.current) clearTimeout(pollTimerRef.current)
    }
  }, [])

  const handleOpenChange = (v: boolean) => {
    if (!v) {
      if (pollTimerRef.current) clearTimeout(pollTimerRef.current)
      setStep('form')
      setSession(null)
      setCredentialId(null)
      setIsStarting(false)
      setIsCompleting(false)
      setCallbackUrl('')
    }
    onOpenChange(v)
  }

  const handleStart = async () => {
    setIsStarting(true)
    const loginWindow = window.open('about:blank', '_blank')
    try {
      const resp = await startSocialLogin({
        priority: parseInt(priority) || 0,
        email: email.trim() || undefined,
      })
      setSession(resp)
      setStep('waiting')
      if (loginWindow) {
        loginWindow.location.href = resp.portalUrl
      } else {
        window.open(resp.portalUrl, '_blank')
      }
      if (!isRemote) schedulePoll(resp.sessionId)
    } catch (e) {
      loginWindow?.close()
      toast.error('发起登录失败：' + extractErrorMessage(e))
    } finally {
      setIsStarting(false)
    }
  }

  const schedulePoll = (sessionId: string) => {
    pollTimerRef.current = setTimeout(async () => {
      try {
        const result = await pollSocialLogin(sessionId)
        if (result.status === 'pending') {
          schedulePoll(sessionId)
        } else if (result.status === 'success') {
          setCredentialId(result.credentialId)
          setStep('done')
          onSuccess()
          toast.success(`登录成功，已添加凭据 #${result.credentialId}`)
        } else {
          toast.error('会话已过期，请重新发起登录')
          setStep('form')
          setSession(null)
        }
      } catch (e) {
        toast.error('轮询失败：' + extractErrorMessage(e))
        schedulePoll(sessionId)
      }
    }, POLL_INTERVAL_MS)
  }

  const handleCompleteManually = async () => {
    if (!session) return
    const parsed = parseCallbackUrl(callbackUrl)
    if (!parsed) {
      toast.error('URL 格式无效，请复制完整的地址栏 URL')
      return
    }
    setIsCompleting(true)
    try {
      const result = await completeSocialLogin(session.sessionId, {
        code: parsed.code,
        state: parsed.state,
        loginOption: parsed.loginOption || undefined,
        path: parsed.path,
      })
      if (result.status === 'success') {
        setCredentialId(result.credentialId)
        setStep('done')
        onSuccess()
        toast.success(`登录成功，已添加凭据 #${result.credentialId}`)
      } else {
        toast.error('会话已过期，请重新发起登录')
        setStep('form')
        setSession(null)
      }
    } catch (e) {
      toast.error('完成登录失败：' + extractErrorMessage(e))
    } finally {
      setIsCompleting(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Kiro 账号登录（Google / GitHub）</DialogTitle>
          <DialogDescription>
            通过 Kiro 网页端完成 Social 登录，无需手动导出 refreshToken。
          </DialogDescription>
        </DialogHeader>

        {step === 'form' && (
          <div className="space-y-4 py-2">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <label htmlFor="social-priority" className="text-sm font-medium">优先级</label>
                <Input
                  id="social-priority"
                  type="number"
                  min="0"
                  value={priority}
                  onChange={(e) => setPriority(e.target.value)}
                />
              </div>
              <div className="space-y-1.5">
                <label htmlFor="social-email" className="text-sm font-medium">邮箱（可选）</label>
                <Input
                  id="social-email"
                  placeholder="user@example.com"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
              </div>
            </div>
          </div>
        )}

        {step === 'waiting' && session && (
          <div className="space-y-4 py-2">
            <div className="rounded-lg border bg-muted/50 p-4 space-y-3">
              <p className="text-sm text-muted-foreground">
                浏览器应已自动打开 Kiro 登录页，请完成授权。
              </p>
              <a
                href={session.portalUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 text-sm font-medium text-primary hover:underline"
              >
                重新打开登录页
                <ExternalLink className="h-3.5 w-3.5" />
              </a>
            </div>

            {isRemote ? (
              // 远程模式：OAuth 回调到 localhost 无法自动捕获，需用户手动复制 URL
              <div className="space-y-2">
                <p className="text-sm text-amber-600 dark:text-amber-400">
                  完成登录后，浏览器会跳转到 <code>localhost</code> 失败页面，
                  请从地址栏复制完整 URL 粘贴到下方：
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

        {step === 'done' && (
          <div className="flex flex-col items-center gap-3 py-4">
            <CheckCircle className="h-10 w-10 text-green-500" />
            <p className="text-sm font-medium">登录成功</p>
            <p className="text-xs text-muted-foreground">凭据 #{credentialId} 已添加并启用</p>
          </div>
        )}

        <DialogFooter>
          {step === 'form' && (
            <Button onClick={handleStart} disabled={isStarting}>
              {isStarting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              发起登录
            </Button>
          )}
          {step === 'waiting' && (
            <>
              <Button variant="outline" onClick={() => handleOpenChange(false)} disabled={isCompleting}>
                取消
              </Button>
              {isRemote && (
                <Button
                  onClick={handleCompleteManually}
                  disabled={isCompleting || !callbackUrl.trim()}
                >
                  {isCompleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                  完成登录
                </Button>
              )}
            </>
          )}
          {step === 'done' && (
            <Button onClick={() => handleOpenChange(false)}>关闭</Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
