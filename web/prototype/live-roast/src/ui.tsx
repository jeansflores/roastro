// PROTOTYPE, throwaway. Small shadcn-flavoured primitives and the form
// fields every variant needs somewhere (where they go is the variant's call).
import { clsx, type ClassValue } from 'clsx'
import { useEffect, type ButtonHTMLAttributes, type ReactNode } from 'react'
import { twMerge } from 'tailwind-merge'
import { COFFEES, REFERENCES, type EventKind } from './lib/roast'
import type { Session } from './lib/session'

export const cn = (...c: ClassValue[]) => twMerge(clsx(c))

type Variant = 'primary' | 'outline' | 'ghost' | 'subtle'
export function Button({ variant = 'outline', className, ...p }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      {...p}
      className={cn(
        'inline-flex items-center justify-center gap-2 rounded-lg px-3 py-2 text-sm font-medium transition-colors disabled:pointer-events-none disabled:opacity-30',
        variant === 'primary' && 'bg-accent text-black hover:bg-accent/90',
        variant === 'outline' && 'border border-line bg-panel-2 text-white/90 hover:border-white/25 hover:bg-white/5',
        variant === 'ghost' && 'text-white/60 hover:bg-white/5 hover:text-white',
        variant === 'subtle' && 'bg-white/5 text-white/85 hover:bg-white/10',
        className,
      )}
    />
  )
}

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <kbd className={cn('rounded border border-white/15 px-1.5 py-0.5 font-mono text-[10px] leading-none text-white/50', className)}>
      {children}
    </kbd>
  )
}

export function Label({ children }: { children: ReactNode }) {
  return <span className="text-[11px] font-medium tracking-wider text-white/40 uppercase">{children}</span>
}

const field = 'w-full rounded-lg border border-line bg-panel-2 px-3 py-2 text-sm text-white outline-none focus:border-accent/60'

export function NextRoastFields({ s, compact }: { s: Session; compact?: boolean }) {
  const set = (k: keyof Session['next']) => (e: { target: { value: string } }) =>
    s.setNext({ ...s.next, [k]: k === 'refId' && e.target.value === '' ? null : e.target.value })
  return (
    <div className={cn('grid gap-3', compact ? 'grid-cols-2' : 'grid-cols-1')}>
      <label className={cn('grid gap-1', compact && 'col-span-2')}>
        <Label>Coffee</Label>
        <select className={field} value={s.next.coffee} onChange={set('coffee')}>
          {COFFEES.map((c) => (
            <option key={c}>{c}</option>
          ))}
        </select>
      </label>
      <label className="grid gap-1">
        <Label>Green weight (kg)</Label>
        <input className={cn(field, 'font-mono')} value={s.next.greenKg} onChange={set('greenKg')} inputMode="decimal" />
      </label>
      <label className="grid gap-1">
        <Label>Roasted by</Label>
        <input className={field} value={s.next.roastedBy} onChange={set('roastedBy')} />
      </label>
      <label className={cn('grid gap-1', compact && 'col-span-2')}>
        <Label>Reference roast</Label>
        <select className={field} value={s.next.refId ?? ''} onChange={set('refId')}>
          {REFERENCES.map((r, i) => (
            <option key={r.id} value={r.id}>
              {r.label} · {r.date}
              {i === 0 ? ' (latest of this coffee)' : ''}
            </option>
          ))}
          <option value="">No reference</option>
        </select>
      </label>
    </div>
  )
}

export function RoastedWeightField({ s, className }: { s: Session; className?: string }) {
  return (
    <label className={cn('grid gap-1', className)}>
      <Label>Roasted weight (kg)</Label>
      <div className="flex items-center gap-3">
        <input
          className={cn(field, 'w-32 font-mono')}
          placeholder="0.000"
          value={s.roastedKg}
          onChange={(e) => s.setRoastedKg(e.target.value)}
          inputMode="decimal"
        />
        <span className="font-mono text-sm text-white/60">
          {s.weightLoss !== undefined ? `−${s.weightLoss.toFixed(1)} % loss` : 'loss —'}
        </span>
      </div>
    </label>
  )
}

const typing = () => {
  const el = document.activeElement as HTMLElement | null
  return !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable)
}

// Keyboard shortcuts are extras: C/D/F/S/X mark, N opens the note input.
export function useHotkeys(s: Session, onNote?: () => void, at?: number) {
  useEffect(() => {
    const keys: Record<string, EventKind> = { c: 'charge', d: 'dry_end', f: 'first_crack', s: 'second_crack', x: 'drop' }
    const h = (e: KeyboardEvent) => {
      if (typing() || e.metaKey || e.ctrlKey || e.altKey) return
      const k = e.key.toLowerCase()
      if (keys[k]) {
        e.preventDefault()
        s.mark(keys[k], keys[k] === 'drop' ? undefined : at)
      } else if (k === 'n' && onNote) {
        e.preventDefault()
        onNote()
      }
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [s, onNote, at])
}

export function StatusChip({ s }: { s: Session }) {
  const map = {
    waiting: ['Waiting for charge', 'bg-white/10 text-white/70'],
    recording: ['Recording', 'bg-accent/15 text-accent'],
    done: ['Done', 'bg-ror/15 text-ror'],
  } as const
  const [text, cls] = map[s.status]
  return (
    <span className={cn('inline-flex items-center gap-2 rounded-full px-3 py-1 text-xs font-semibold tracking-wide uppercase', cls)}>
      {s.status === 'recording' && <span className="size-2 animate-pulse rounded-full bg-accent" />}
      {text}
    </span>
  )
}
