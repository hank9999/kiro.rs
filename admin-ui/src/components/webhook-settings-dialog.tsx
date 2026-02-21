import { useState, useEffect } from 'react'
import { toast } from 'sonner'
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
import { useWebhookUrl, useSetWebhookUrl, useTestWebhook } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'

interface WebhookSettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

// 预设模板
const PRESETS: { label: string; body: string }[] = [
  {
    label: '默认格式',
    body: '',
  },
  {
    label: '企业微信',
    body: JSON.stringify({
      msgtype: 'text',
      text: {
        content: '⚠️ Kiro 凭据告警\n凭据 #{{credential_id}}（{{email}}）已被禁用\n原因: {{reason_zh}}\n剩余可用: {{available}}/{{total}}\n时间: {{timestamp}}',
      },
    }, null, 2),
  },
  {
    label: 'Slack',
    body: JSON.stringify({
      text: '⚠️ *Kiro Alert*: 凭据 #{{credential_id}}（{{email}}）已被禁用\n原因: {{reason_zh}} | 剩余: {{available}}/{{total}}',
    }, null, 2),
  },
  {
    label: '飞书',
    body: JSON.stringify({
      msg_type: 'text',
      content: {
        text: '⚠️ Kiro 凭据告警\n凭据 #{{credential_id}}（{{email}}）已被禁用\n原因: {{reason_zh}}\n剩余可用: {{available}}/{{total}}',
      },
    }, null, 2),
  },
]
const VARIABLES = [
  ['{{credential_id}}', '凭据 ID'],
  ['{{email}}', '邮箱'],
  ['{{reason}}', '原因(英文)'],
  ['{{reason_zh}}', '原因(中文)'],
  ['{{available}}', '可用数'],
  ['{{total}}', '总数'],
  ['{{timestamp}}', '时间'],
]

export function WebhookSettingsDialog({ open, onOpenChange }: WebhookSettingsDialogProps) {
  const [urlInput, setUrlInput] = useState('')
  const [bodyInput, setBodyInput] = useState('')
  const [activePreset, setActivePreset] = useState(0)

  const { data: webhookData } = useWebhookUrl()
  const { mutate: setWebhookUrl, isPending } = useSetWebhookUrl()
  const { mutate: sendTest, isPending: isTesting } = useTestWebhook()

  // 加载已有配置
  useEffect(() => {
    if (open && webhookData) {
      setUrlInput(webhookData.url || '')
      setBodyInput(webhookData.body || '')
      // 匹配预设
      const idx = PRESETS.findIndex(p => p.body === (webhookData.body || ''))
      setActivePreset(idx >= 0 ? idx : -1)
    }
  }, [open, webhookData])

  const handlePresetSelect = (index: number) => {
    setActivePreset(index)
    setBodyInput(PRESETS[index].body)
  }

  const handleTest = () => {
    const url = urlInput.trim()
    if (!url) {
      toast.error('请填写 Webhook URL')
      return
    }
    sendTest({ url, body: bodyInput.trim() || null }, {
      onSuccess: () => {
        toast.success('测试 Webhook 发送成功')
      },
      onError: (error) => {
        toast.error(`测试失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  const handleSave = () => {
    const url = urlInput.trim() || null
    const body = bodyInput.trim() || null
    setWebhookUrl({ url, body }, {
      onSuccess: () => {
        toast.success(url ? 'Webhook 配置已保存' : 'Webhook 通知已禁用')
        onOpenChange(false)
      },
      onError: (error) => {
        toast.error(`保存失败: ${extractErrorMessage(error)}`)
      },
    })
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Webhook 通知配置</DialogTitle>
          <DialogDescription>
            凭据被禁用时，自动发送 HTTP POST 通知到指定 URL。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* URL */}
          <div className="space-y-2">
            <label className="text-sm font-medium">Webhook URL</label>
            <Input
              placeholder="https://hooks.example.com/your-webhook-endpoint"
              value={urlInput}
              onChange={(e) => setUrlInput(e.target.value)}
            />
          </div>

          {/* 预设模板选择 */}
          <div className="space-y-2">
            <label className="text-sm font-medium">JSON 模板</label>
            <div className="flex flex-wrap gap-2">
              {PRESETS.map((preset, i) => (
                <Button
                  key={i}
                  variant={activePreset === i ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => handlePresetSelect(i)}
                >
                  {preset.label}
                </Button>
              ))}
              <Button
                variant={activePreset === -1 ? 'default' : 'outline'}
                size="sm"
                onClick={() => setActivePreset(-1)}
              >
                自定义
              </Button>
            </div>
          </div>

          {/* 模板编辑器 */}
          <div className="space-y-2">
            <textarea
              className="w-full min-h-[160px] rounded-md border border-input bg-background px-3 py-2 text-sm font-mono ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              placeholder='留空使用默认格式，或输入自定义 JSON 模板'
              value={bodyInput}
              onChange={(e) => {
                setBodyInput(e.target.value)
                setActivePreset(-1)
              }}
            />
            <div className="flex flex-wrap gap-1">
              {VARIABLES.map(([v, label]) => (
                <span key={v} className="text-xs bg-muted px-1.5 py-0.5 rounded" title={label}>
                  {v}
                </span>
              ))}
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button
            variant="outline"
            onClick={handleTest}
            disabled={isTesting || !urlInput.trim()}
          >
            {isTesting ? '测试中...' : '发送测试'}
          </Button>
          <Button onClick={handleSave} disabled={isPending}>
            {isPending ? '保存中...' : '保存'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
