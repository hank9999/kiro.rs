import { useState } from 'react'
import { toast } from 'sonner'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
  DialogDescription,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { useUpdateRefreshToken, useSetDisabled, useResetFailure, useUpdateCredential } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { CredentialStatusItem } from '@/types/api'

interface UpdateTokenDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  credential: CredentialStatusItem
}

interface ParsedTokenData {
  refreshToken: string
  email?: string
  accessToken?: string
  expiresAt?: string
}

// 从 KAM JSON 或纯字符串中提取 token 相关字段
function parseTokenInput(input: string): ParsedTokenData {
  const trimmed = input.trim()
  if (!trimmed) return { refreshToken: '' }

  try {
    const parsed = JSON.parse(trimmed)

    const extractFromObj = (obj: Record<string, unknown>): ParsedTokenData | null => {
      const rt = typeof obj.refreshToken === 'string' ? obj.refreshToken.trim() : ''
      if (!rt) return null
      const email = typeof obj.email === 'string' ? obj.email.trim() : undefined
      const accessToken = typeof obj.accessToken === 'string' ? obj.accessToken.trim() : undefined
      const expiresAt = typeof obj.expiresAt === 'string' ? obj.expiresAt.trim() : undefined
      return {
        refreshToken: rt,
        email: email || undefined,
        accessToken: accessToken || undefined,
        expiresAt: expiresAt || undefined,
      }
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
      if (first.credentials) {
        const nested = extractFromObj(first.credentials as Record<string, unknown>)
        if (nested) {
          const outerEmail = typeof first.email === 'string'
            ? (first.email as string).trim() || undefined
            : undefined
          return { ...nested, email: nested.email ?? outerEmail }
        }
      }
    }

    return { refreshToken: '' }
  } catch {
    return { refreshToken: trimmed }
  }
}

export function UpdateTokenDialog({ open, onOpenChange, credential }: UpdateTokenDialogProps) {
  const [input, setInput] = useState('')
  const [step, setStep] = useState<'idle' | 'updating' | 'done'>('idle')
  const [stepLog, setStepLog] = useState<string[]>([])

  const updateRefreshToken = useUpdateRefreshToken()
  const updateCredential = useUpdateCredential()
  const setDisabled = useSetDisabled()
  const resetFailure = useResetFailure()

  const parsed = parseTokenInput(input)
  const extractedToken = parsed.refreshToken
  const extractedEmail = parsed.email
  const isValid = extractedToken.length >= 100 && !extractedToken.includes('...')
  const isPending = step === 'updating'

  const addLog = (msg: string) => setStepLog(prev => [...prev, msg])

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!isValid) {
      toast.error('refreshToken 无效或已被截断')
      return
    }

    setStep('updating')
    setStepLog([])

    try {
      // 步骤 1：若凭据未禁用，先禁用（后端要求更新 Token 前必须禁用）
      if (!credential.disabled) {
        addLog('正在临时禁用凭据…')
        await new Promise<void>((resolve, reject) => {
          setDisabled.mutate(
            { id: credential.id, disabled: true },
            { onSuccess: () => resolve(), onError: reject }
          )
        })
        addLog('✓ 已临时禁用')
      }

      // 步骤 2：更新 refreshToken（若 JSON 中含 accessToken 则一并保留，避免立即调认证服务器）
      addLog('正在更新 refreshToken…')
      await new Promise<void>((resolve, reject) => {
        updateRefreshToken.mutate(
          {
            id: credential.id,
            req: {
              refreshToken: extractedToken,
              accessToken: parsed.accessToken,
              expiresAt: parsed.expiresAt,
            },
          },
          { onSuccess: () => resolve(), onError: reject }
        )
      })
      addLog(`✓ refreshToken 已更新${parsed.accessToken ? '（含 accessToken）' : ''}`)

      // 步骤 3：重置失败计数
      addLog('正在重置失败计数…')
      await new Promise<void>((resolve, reject) => {
        resetFailure.mutate(credential.id, {
          onSuccess: () => resolve(),
          onError: reject,
        })
      })
      addLog('✓ 失败计数已重置')

      // 步骤 4：启用凭据
      addLog('正在重新启用凭据…')
      await new Promise<void>((resolve, reject) => {
        setDisabled.mutate(
          { id: credential.id, disabled: false },
          { onSuccess: () => resolve(), onError: reject }
        )
      })
      addLog('✓ 凭据已启用')

      // 步骤 5：如果 JSON 中包含 email 且与当前不同，同步更新
      if (extractedEmail && extractedEmail !== credential.email) {
        addLog(`正在更新邮箱为 ${extractedEmail}…`)
        await new Promise<void>((resolve, reject) => {
          updateCredential.mutate(
            { id: credential.id, req: { email: extractedEmail } },
            { onSuccess: () => resolve(), onError: reject }
          )
        })
        addLog(`✓ 邮箱已更新为 ${extractedEmail}`)
      }

      setStep('done')
      toast.success(`凭据 #${credential.id} 重新导入完成，已自动启用`)
    } catch (error) {
      addLog(`✗ 失败: ${extractErrorMessage(error)}`)
      setStep('idle')
      toast.error(`重新导入失败: ${extractErrorMessage(error)}`)
    }
  }

  const handleClose = () => {
    if (isPending) return
    setInput('')
    setStep('idle')
    setStepLog([])
    onOpenChange(false)
  }

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>重新导入凭据 #{credential.id}</DialogTitle>
          <DialogDescription>
            为 {credential.email || `凭据 #${credential.id}`} 粘贴新 Token，
            系统将自动更新 Token、重置失败计数并重新启用。
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={handleSubmit}>
          <div className="space-y-4 py-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">
                粘贴 KAM 导出 JSON 或直接粘贴 refreshToken 字符串
              </label>
              <textarea
                placeholder={'支持以下格式：\n\n1. 直接粘贴 refreshToken 字符串\n\n2. KAM 导出的单账号 JSON：\n{\n  "email": "...",\n  "refreshToken": "aor...",\n  "authMethod": "social"\n}'}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                disabled={isPending || step === 'done'}
                className="flex min-h-[140px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50 font-mono"
              />
            </div>

            {/* Token 解析预览 */}
            {input.trim() && step === 'idle' && (
              <div className={`text-sm rounded-md p-3 ${isValid ? 'bg-green-50 dark:bg-green-950 text-green-700 dark:text-green-300' : 'bg-red-50 dark:bg-red-950 text-red-700 dark:text-red-300'}`}>
                {isValid ? (
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

            {/* 执行步骤日志 */}
            {stepLog.length > 0 && (
              <div className="rounded-md border bg-muted/40 p-3 space-y-1">
                {stepLog.map((log, i) => (
                  <div key={i} className="text-sm font-mono">
                    {log}
                  </div>
                ))}
              </div>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={handleClose} disabled={isPending}>
              {step === 'done' ? '关闭' : '取消'}
            </Button>
            {step !== 'done' && (
              <Button type="submit" disabled={isPending || !isValid}>
                {isPending ? '处理中…' : '重新导入并启用'}
              </Button>
            )}
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
