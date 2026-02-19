import { useState, useEffect } from 'react'
import { toast } from 'sonner'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useEmailConfig, useUpdateEmailConfig, useDeleteEmailConfig, useSendTestEmail } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'

interface EmailConfigDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function EmailConfigDialog({ open, onOpenChange }: EmailConfigDialogProps) {
  const [smtpHost, setSmtpHost] = useState('')
  const [smtpPort, setSmtpPort] = useState('587')
  const [smtpUsername, setSmtpUsername] = useState('')
  const [smtpPassword, setSmtpPassword] = useState('')
  const [fromAddress, setFromAddress] = useState('')
  const [recipients, setRecipients] = useState('')

  const { data: emailConfig, isLoading } = useEmailConfig()
  const { mutate: updateConfig, isPending: isUpdating } = useUpdateEmailConfig()
  const { mutate: deleteConfig, isPending: isDeleting } = useDeleteEmailConfig()
  const { mutate: sendTestEmail, isPending: isSending } = useSendTestEmail()

  const isPending = isUpdating || isDeleting || isSending

  // 打开时加载现有配置
  useEffect(() => {
    if (open && emailConfig?.configured) {
      setSmtpHost(emailConfig.smtpHost || '')
      setSmtpPort(String(emailConfig.smtpPort || 587))
      setSmtpUsername(emailConfig.smtpUsername || '')
      setSmtpPassword('')
      setFromAddress(emailConfig.fromAddress || '')
      setRecipients(emailConfig.recipients?.join(', ') || '')
    } else if (open && emailConfig && !emailConfig.configured) {
      setSmtpHost('')
      setSmtpPort('587')
      setSmtpUsername('')
      setSmtpPassword('')
      setFromAddress('')
      setRecipients('')
    }
  }, [open, emailConfig])

  const handleSave = (e: React.FormEvent) => {
    e.preventDefault()

    if (!smtpHost.trim() || !smtpUsername.trim() || !smtpPassword.trim() || !fromAddress.trim() || !recipients.trim()) {
      toast.error('请填写所有必填字段')
      return
    }

    const recipientList = recipients.split(',').map(r => r.trim()).filter(Boolean)
    if (recipientList.length === 0) {
      toast.error('请至少填写一个收件人')
      return
    }

    updateConfig(
      {
        smtpHost: smtpHost.trim(),
        smtpPort: parseInt(smtpPort) || 587,
        smtpUsername: smtpUsername.trim(),
        smtpPassword: smtpPassword.trim(),
        fromAddress: fromAddress.trim(),
        recipients: recipientList,
      },
      {
        onSuccess: () => {
          toast.success('邮件配置已保存')
          onOpenChange(false)
        },
        onError: (error: unknown) => {
          toast.error(`保存失败: ${extractErrorMessage(error)}`)
        },
      }
    )
  }

  const handleDelete = () => {
    if (!confirm('确定要删除邮件配置吗？删除后将无法发送通知邮件。')) {
      return
    }

    deleteConfig(undefined, {
      onSuccess: () => {
        toast.success('邮件配置已删除')
        onOpenChange(false)
      },
      onError: (error: unknown) => {
        toast.error(`删除失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  const handleTestEmail = () => {
    sendTestEmail(undefined, {
      onSuccess: () => {
        toast.success('测试邮件已发送')
      },
      onError: (error: unknown) => {
        toast.error(`发送失败: ${extractErrorMessage(error)}`)
      },
    })
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg max-h-[85vh] flex flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>邮件通知配置</DialogTitle>
        </DialogHeader>

        {isLoading ? (
          <div className="py-8 text-center text-muted-foreground">加载中...</div>
        ) : (
          <form onSubmit={handleSave} className="flex flex-col min-h-0 flex-1 overflow-hidden">
            <div className="space-y-4 py-4 overflow-y-auto flex-1 px-2 -mx-2">
              <div className="space-y-2">
                <label htmlFor="smtpHost" className="text-sm font-medium">
                  SMTP 服务器 <span className="text-red-500">*</span>
                </label>
                <div className="grid grid-cols-3 gap-2">
                  <Input
                    id="smtpHost"
                    placeholder="smtp.example.com"
                    value={smtpHost}
                    onChange={(e) => setSmtpHost(e.target.value)}
                    disabled={isPending}
                    className="col-span-2"
                  />
                  <Input
                    id="smtpPort"
                    type="number"
                    placeholder="587"
                    value={smtpPort}
                    onChange={(e) => setSmtpPort(e.target.value)}
                    disabled={isPending}
                  />
                </div>
              </div>

              <div className="space-y-2">
                <label htmlFor="smtpUsername" className="text-sm font-medium">
                  SMTP 用户名 <span className="text-red-500">*</span>
                </label>
                <Input
                  id="smtpUsername"
                  placeholder="user@example.com"
                  value={smtpUsername}
                  onChange={(e) => setSmtpUsername(e.target.value)}
                  disabled={isPending}
                />
              </div>

              <div className="space-y-2">
                <label htmlFor="smtpPassword" className="text-sm font-medium">
                  SMTP 密码 <span className="text-red-500">*</span>
                </label>
                <Input
                  id="smtpPassword"
                  type="password"
                  placeholder={emailConfig?.configured ? '留空保持不变' : '请输入 SMTP 密码'}
                  value={smtpPassword}
                  onChange={(e) => setSmtpPassword(e.target.value)}
                  disabled={isPending}
                />
              </div>
              <div className="space-y-2">
                <label htmlFor="fromAddress" className="text-sm font-medium">
                  发件人地址 <span className="text-red-500">*</span>
                </label>
                <Input
                  id="fromAddress"
                  placeholder='kiro-rs <noreply@example.com>'
                  value={fromAddress}
                  onChange={(e) => setFromAddress(e.target.value)}
                  disabled={isPending}
                />
              </div>

              <div className="space-y-2">
                <label htmlFor="recipients" className="text-sm font-medium">
                  收件人 <span className="text-red-500">*</span>
                </label>
                <Input
                  id="recipients"
                  placeholder="多个收件人用逗号分隔"
                  value={recipients}
                  onChange={(e) => setRecipients(e.target.value)}
                  disabled={isPending}
                />
                <p className="text-xs text-muted-foreground">
                  多个收件人用逗号分隔，如: admin@example.com, ops@example.com
                </p>
              </div>
            </div>

            <DialogFooter className="flex-row justify-between sm:justify-between flex-shrink-0">
              <div className="flex gap-2">
                {emailConfig?.configured && (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={handleTestEmail}
                      disabled={isPending}
                    >
                      {isSending ? '发送中...' : '测试邮件'}
                    </Button>
                    <Button
                      type="button"
                      variant="destructive"
                      size="sm"
                      onClick={handleDelete}
                      disabled={isPending}
                    >
                      {isDeleting ? '删除中...' : '删除配置'}
                    </Button>
                  </>
                )}
              </div>
              <div className="flex gap-2">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => onOpenChange(false)}
                  disabled={isPending}
                >
                  取消
                </Button>
                <Button type="submit" disabled={isPending}>
                  {isUpdating ? '保存中...' : '保存'}
                </Button>
              </div>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
}