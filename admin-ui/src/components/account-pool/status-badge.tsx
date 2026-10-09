import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import type { CredentialHealth } from '@/lib/credential-status'

interface StatusBadgeProps {
  health: CredentialHealth
  className?: string
}

const toneClassNames: Record<CredentialHealth['tone'], string> = {
  green:
    'border-transparent bg-emerald-100 text-emerald-700 hover:bg-emerald-100 dark:bg-emerald-950 dark:text-emerald-300',
  blue:
    'border-transparent bg-sky-100 text-sky-700 hover:bg-sky-100 dark:bg-sky-950 dark:text-sky-300',
  yellow:
    'border-transparent bg-amber-100 text-amber-700 hover:bg-amber-100 dark:bg-amber-950 dark:text-amber-300',
  orange:
    'border-transparent bg-orange-100 text-orange-700 hover:bg-orange-100 dark:bg-orange-950 dark:text-orange-300',
  red:
    'border-transparent bg-red-100 text-red-700 hover:bg-red-100 dark:bg-red-950 dark:text-red-300',
  gray:
    'border-transparent bg-slate-100 text-slate-600 hover:bg-slate-100 dark:bg-slate-900 dark:text-slate-300',
}

export function StatusBadge({ health, className }: StatusBadgeProps) {
  return (
    <Badge
      variant="outline"
      className={cn('whitespace-nowrap font-medium', toneClassNames[health.tone], className)}
      title={health.description}
    >
      {health.label}
    </Badge>
  )
}
