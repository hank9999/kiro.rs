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
import { Switch } from '@/components/ui/switch'
import { useEmailConfig, useSaveEmailConfig, useTestEmail } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'

interface EmailSettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function EmailSettingsDialog({ open, onOpenChange }: EmailSettingsDialogProps) {
  const [enabled, setEnabled] = useState(false)
  const [smtpHost, setSmtpHost] = useState('')
  const [smtpPort, setSmtpPort] = useState('587')
  const [smtpUsername, setSmtpUsername] = useState('')
  const [smtpPassword, setSmtpPassword] = useState('')
  const [smtpTls, setSmtpTls] = useState(true)
  const [fromAddress, setFromAddress] = useState('')
  const [toAddresses, setToAddresses] = useState('')
  const [testPassed, setTestPassed] = useState(false)

  const { data: emailConfig } = useEmailConfig()
  const { mutate: saveConfig, isPending: isSaving } = useSaveEmailConfig()
  const { mutate: sendTest, isPending: isTesting } = useTestEmail()

  // 加载已有配置
  useEffect(() => {
    if (emailConfig) {
      setEnabled(emailConfig.enabled)
      setSmtpHost(emailConfig.smtpHost)
      setSmtpPort(String(emailConfig.smtpPort))
      setSmtpUsername(emailConfig.smtpUsername)
      setSmtpTls(emailConfig.smtpTls)
      setFromAddress(emailConfig.fromAddress)
      setToAddresses(emailConfig.toAddresses.join(', '))
      setSmtpPassword('')
      setTestPassed(false)
    }
  }, [emailConfig, open])

  // 表单变更时重置测试状态
  const handleFieldChange = () => {
    setTestPassed(false)
  }

  const parseToAddresses = (): string[] => {
    return toAddresses
      .split(/[,;，；\n]/)
      .map(s => s.trim())
      .filter(s => s.length > 0)
  }

  const getEffectivePassword = (): string => {
    // 如果用户输入了新密码，使用新密码；否则需要已有密码
    if (smtpPassword) return smtpPassword
    if (emailConfig?.smtpPasswordSet) return '' // 空字符串表示保留原密码
    return ''
  }

  const handleTest = () => {
    const addrs = parseToAddresses()
    if (!smtpHost || !smtpUsername || addrs.length === 0 || !fromAddress) {
      toast.error('请填写完整的 SMTP 配置')
      return
    }

    // 测试时必须有密码（新输入的或已保存的）
    const password = smtpPassword
    if (!password && !emailConfig?.smtpPasswordSet) {
      toast.error('请输入 SMTP 密码')
      return
    }
    if (!password && emailConfig?.smtpPasswordSet) {
      toast.error('测试发送需要重新输入密码')
      return
    }

    sendTest({
      smtpHost,
      smtpPort: parseInt(smtpPort) || 587,
      smtpUsername,
      smtpPassword: password,
      smtpTls,
      fromAddress,
      toAddresses: addrs,
    }, {
      onSuccess: () => {
        toast.success('测试邮件发送成功')
        setTestPassed(true)
      },
      onError: (error) => {
        toast.error(`测试失败: ${extractErrorMessage(error)}`)
        setTestPassed(false)
      },
    })
  }

  const handleSave = () => {
    const addrs = parseToAddresses()
    saveConfig({
      enabled,
      smtpHost,
      smtpPort: parseInt(smtpPort) || 587,
      smtpUsername,
      smtpPassword: getEffectivePassword(),
      smtpTls,
      fromAddress,
      toAddresses: addrs,
    }, {
      onSuccess: () => {
        toast.success('邮件配置已保存')
        onOpenChange(false)
      },
      onError: (error) => {
        toast.error(`保存失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[500px] max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>邮件通知设置</DialogTitle>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {/* 启用开关 */}
          <div className="flex items-center justify-between">
            <label className="text-sm font-medium">启用邮件通知</label>
            <Switch checked={enabled} onCheckedChange={(v) => { setEnabled(v); handleFieldChange() }} />
          </div>

          {/* SMTP 服务器 */}
          <div className="space-y-2">
            <label className="text-sm font-medium">SMTP 服务器</label>
            <Input
              placeholder="smtp.example.com"
              value={smtpHost}
              onChange={(e) => { setSmtpHost(e.target.value); handleFieldChange() }}
            />
          </div>

          {/* 端口 + STARTTLS */}
          <div className="flex gap-4 items-end">
            <div className="flex-1 space-y-2">
              <label className="text-sm font-medium">端口</label>
              <Input
                type="number"
                placeholder="587"
                value={smtpPort}
                onChange={(e) => { setSmtpPort(e.target.value); handleFieldChange() }}
              />
            </div>
            <div className="flex items-center gap-2 pb-2">
              <Switch checked={smtpTls} onCheckedChange={(v) => { setSmtpTls(v); handleFieldChange() }} />
              <label className="text-sm">STARTTLS</label>
            </div>
          </div>

          {/* 用户名 */}
          <div className="space-y-2">
            <label className="text-sm font-medium">用户名</label>
            <Input
              placeholder="user@example.com"
              value={smtpUsername}
              onChange={(e) => { setSmtpUsername(e.target.value); handleFieldChange() }}
            />
          </div>

          {/* 密码 */}
          <div className="space-y-2">
            <label className="text-sm font-medium">密码</label>
            <Input
              type="password"
              placeholder={emailConfig?.smtpPasswordSet ? '留空保留原密码' : '输入 SMTP 密码'}
              value={smtpPassword}
              onChange={(e) => { setSmtpPassword(e.target.value); handleFieldChange() }}
            />
          </div>

          {/* 发件人 */}
          <div className="space-y-2">
            <label className="text-sm font-medium">发件人地址</label>
            <Input
              placeholder="noreply@example.com"
              value={fromAddress}
              onChange={(e) => { setFromAddress(e.target.value); handleFieldChange() }}
            />
          </div>

          {/* 收件人 */}
          <div className="space-y-2">
            <label className="text-sm font-medium">收件人地址</label>
            <Input
              placeholder="admin@example.com, ops@example.com"
              value={toAddresses}
              onChange={(e) => { setToAddresses(e.target.value); handleFieldChange() }}
            />
            <p className="text-xs text-muted-foreground">多个地址用逗号分隔</p>
          </div>
        </div>

        <DialogFooter className="flex gap-2">
          <Button
            variant="outline"
            onClick={handleTest}
            disabled={isTesting}
          >
            {isTesting ? '发送中...' : '发送测试邮件'}
          </Button>
          <Button
            onClick={handleSave}
            disabled={!testPassed || isSaving}
          >
            {isSaving ? '保存中...' : '保存'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}