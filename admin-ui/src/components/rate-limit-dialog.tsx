import { useEffect, useState } from 'react'
import { toast } from 'sonner'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import type { RateLimitRule } from '@/types/api'

interface EditableRateLimitRule {
  window: string
  maxRequests: string
}

interface RateLimitDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description: string
  initialRules?: RateLimitRule[]
  loading?: boolean
  onSave: (rules?: RateLimitRule[]) => void
}

function toEditableRules(rules?: RateLimitRule[]): EditableRateLimitRule[] {
  if (!rules || rules.length === 0) {
    return [{ window: '', maxRequests: '' }]
  }

  return rules.map((rule) => ({
    window: rule.window,
    maxRequests: String(rule.maxRequests),
  }))
}

export function RateLimitDialog({
  open,
  onOpenChange,
  title,
  description,
  initialRules,
  loading = false,
  onSave,
}: RateLimitDialogProps) {
  const [rules, setRules] = useState<EditableRateLimitRule[]>(toEditableRules(initialRules))

  useEffect(() => {
    if (open) {
      setRules(toEditableRules(initialRules))
    }
  }, [initialRules, open])

  const updateRule = (index: number, key: keyof EditableRateLimitRule, value: string) => {
    setRules((prev) => prev.map((rule, idx) => (
      idx === index ? { ...rule, [key]: value } : rule
    )))
  }

  const handleSave = () => {
    const normalized = rules
      .map((rule) => ({
        window: rule.window.trim(),
        maxRequests: rule.maxRequests.trim(),
      }))
      .filter((rule) => rule.window !== '' || rule.maxRequests !== '')

    if (normalized.some((rule) => rule.window === '' || rule.maxRequests === '')) {
      toast.error('时间窗口和最大请求数必须同时填写')
      return
    }

    const parsed = normalized.map((rule) => ({
      window: rule.window,
      maxRequests: Number.parseInt(rule.maxRequests, 10),
    }))

    if (parsed.some((rule) => Number.isNaN(rule.maxRequests) || rule.maxRequests <= 0)) {
      toast.error('最大请求数必须是正整数')
      return
    }

    onSave(parsed.length > 0 ? parsed : undefined)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>

        <div className="space-y-3">
          {rules.map((rule, index) => (
            <div key={index} className="grid grid-cols-[1fr_1fr_auto] gap-2">
              <Input
                value={rule.window}
                onChange={(e) => updateRule(index, 'window', e.target.value)}
                placeholder="例如 5m、30s、2h"
                disabled={loading}
              />
              <Input
                type="number"
                min="1"
                value={rule.maxRequests}
                onChange={(e) => updateRule(index, 'maxRequests', e.target.value)}
                placeholder="最大请求数"
                disabled={loading}
              />
              <Button
                variant="outline"
                onClick={() => {
                  setRules((prev) => {
                    if (prev.length === 1) {
                      return [{ window: '', maxRequests: '' }]
                    }
                    return prev.filter((_, idx) => idx !== index)
                  })
                }}
                disabled={loading}
              >
                删除
              </Button>
            </div>
          ))}

          <Button
            variant="outline"
            onClick={() => setRules((prev) => [...prev, { window: '', maxRequests: '' }])}
            disabled={loading}
          >
            添加规则
          </Button>
          <p className="text-xs text-muted-foreground">
            支持自定义窗口，例如 `30s`、`5m`、`2h`、`1d`。留空并保存可清空当前覆盖规则。
          </p>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={loading}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={loading}>
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
