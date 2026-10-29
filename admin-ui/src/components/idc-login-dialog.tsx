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
import { startIdcLogin, pollIdcLogin } from '@/api/credentials'
import type { StartIdcLoginResponse } from '@/types/api'
import { extractErrorMessage } from '@/lib/utils'

interface IdcLoginDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSuccess: () => void
}

type Step = 'form' | 'waiting' | 'done'

export function IdcLoginDialog({ open, onOpenChange, onSuccess }: IdcLoginDialogProps) {
  const [step, setStep] = useState<Step>('form')
  const [region, setRegion] = useState('us-east-1')
  const [startUrl, setStartUrl] = useState('')
  const [priority, setPriority] = useState('0')
  const [email, setEmail] = useState('')
  const [isStarting, setIsStarting] = useState(false)
  const [session, setSession] = useState<StartIdcLoginResponse | null>(null)
  const [credentialId, setCredentialId] = useState<number | null>(null)
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // 清理轮询定时器
  useEffect(() => {
    return () => {
      if (pollTimerRef.current) clearTimeout(pollTimerRef.current)
    }
  }, [])

  // 对话框关闭时重置状态
  const handleOpenChange = (v: boolean) => {
    if (!v) {
      if (pollTimerRef.current) clearTimeout(pollTimerRef.current)
      setStep('form')
      setSession(null)
      setCredentialId(null)
      setIsStarting(false)
    }
    onOpenChange(v)
  }

  const handleStart = async () => {
    if (!region.trim()) {
      toast.error('请填写 AWS Region')
      return
    }
    setIsStarting(true)
    try {
      const resp = await startIdcLogin({
        region: region.trim(),
        startUrl: startUrl.trim() || undefined,
        priority: parseInt(priority) || 0,
        email: email.trim() || undefined,
      })
      setSession(resp)
      setStep('waiting')
      schedulePoll(resp.sessionId, resp.pollInterval)
    } catch (e) {
      toast.error('发起登录失败：' + extractErrorMessage(e))
    } finally {
      setIsStarting(false)
    }
  }

  const schedulePoll = (sessionId: string, interval: number) => {
    pollTimerRef.current = setTimeout(async () => {
      try {
        const result = await pollIdcLogin(sessionId)
        if (result.status === 'pending') {
          schedulePoll(sessionId, interval)
        } else if (result.status === 'success') {
          setCredentialId(result.credentialId)
          setStep('done')
          onSuccess()
          toast.success(`登录成功，已添加凭据 #${result.credentialId}`)
        } else {
          toast.error('授权已过期，请重新发起登录')
          setStep('form')
          setSession(null)
        }
      } catch (e) {
        toast.error('轮询状态失败：' + extractErrorMessage(e))
        schedulePoll(sessionId, interval)
      }
    }, interval * 1000)
  }

  const copyCode = () => {
    if (!session) return
    navigator.clipboard.writeText(session.userCode)
    toast.success('验证码已复制')
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>AWS SSO / Builder ID 登录</DialogTitle>
          <DialogDescription>
            通过 AWS Identity Center 设备授权流程添加凭据，无需手动导出 refreshToken。
          </DialogDescription>
        </DialogHeader>

        {step === 'form' && (
          <div className="space-y-4 py-2">
            <div className="space-y-1.5">
              <label htmlFor="idc-region" className="text-sm font-medium">AWS Region</label>
              <Input
                id="idc-region"
                placeholder="us-east-1"
                value={region}
                onChange={(e) => setRegion(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <label htmlFor="idc-start-url" className="text-sm font-medium">
                SSO Start URL
                <span className="ml-1 text-xs text-muted-foreground">
                  （留空使用 AWS Builder ID）
                </span>
              </label>
              <Input
                id="idc-start-url"
                placeholder="https://view.awsapps.com/start"
                value={startUrl}
                onChange={(e) => setStartUrl(e.target.value)}
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1.5">
                <label htmlFor="idc-priority" className="text-sm font-medium">优先级</label>
                <Input
                  id="idc-priority"
                  type="number"
                  min="0"
                  value={priority}
                  onChange={(e) => setPriority(e.target.value)}
                />
              </div>
              <div className="space-y-1.5">
                <label htmlFor="idc-email" className="text-sm font-medium">邮箱（可选）</label>
                <Input
                  id="idc-email"
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
            <div className="rounded-lg border bg-muted/50 p-4 text-center space-y-3">
              <p className="text-sm text-muted-foreground">在浏览器中访问以下地址并输入验证码</p>
              <a
                href={session.verificationUriComplete ?? session.verificationUri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 text-sm font-medium text-primary hover:underline"
              >
                {session.verificationUri}
                <ExternalLink className="h-3.5 w-3.5" />
              </a>
              <div className="flex items-center justify-center gap-2">
                <span className="font-mono text-2xl font-bold tracking-widest">
                  {session.userCode}
                </span>
                <Button variant="ghost" size="icon" className="h-7 w-7" onClick={copyCode}>
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

        {step === 'done' && (
          <div className="flex flex-col items-center gap-3 py-4">
            <CheckCircle className="h-10 w-10 text-green-500" />
            <p className="text-sm font-medium">登录成功</p>
            <p className="text-xs text-muted-foreground">
              凭据 #{credentialId} 已添加并启用
            </p>
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
            <Button variant="outline" onClick={() => handleOpenChange(false)}>
              取消
            </Button>
          )}
          {step === 'done' && (
            <Button onClick={() => handleOpenChange(false)}>关闭</Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
