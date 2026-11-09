// PROTOTYPE, throwaway. Floating bar to flip variants (← / →) and drive the
// replay. Deliberately styled unlike the app so it reads as scaffolding.
import { useEffect } from 'react'
import { LIVE, fmtClock } from './lib/roast'
import type { Session } from './lib/session'

const JUMPS: [string, number][] = [
  ['Preheat', -90],
  ['Charge', 0],
  ['Turning point', 75],
  ['Before dry end', LIVE.truth.dry_end - 25],
  ['Before first crack', LIVE.truth.first_crack - 25],
  ['Before drop', LIVE.drop_t - 20],
  ['After drop', LIVE.drop_t + 30],
]

export function PrototypeSwitcher({
  variants,
  current,
  onChange,
  s,
}: {
  variants: { key: string; name: string }[]
  current: string
  onChange: (key: string) => void
  s: Session
}) {
  const i = Math.max(0, variants.findIndex((v) => v.key === current))
  const go = (d: number) => onChange(variants[(i + d + variants.length) % variants.length].key)

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const el = document.activeElement as HTMLElement | null
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable)) return
      if (e.key === 'ArrowLeft') go(-1)
      else if (e.key === 'ArrowRight') go(1)
      else if (e.key === ' ') {
        e.preventDefault()
        s.setPlaying(!s.playing)
      }
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  })

  if (!import.meta.env.DEV) return null
  const btn = 'rounded-full px-2.5 py-1 hover:bg-black/10'
  return (
    <div className="fixed bottom-3 left-1/2 z-50 flex -translate-x-1/2 items-center gap-1 rounded-full bg-white px-2 py-1.5 font-sans text-[13px] text-black shadow-2xl shadow-black ring-1 ring-black/10">
      <button className={btn} onClick={() => go(-1)} title="Previous variant (←)">
        ◀
      </button>
      <span className="min-w-44 text-center font-semibold">
        {variants[i].key} · {variants[i].name}
      </span>
      <button className={btn} onClick={() => go(1)} title="Next variant (→)">
        ▶
      </button>
      <span className="mx-1 h-5 w-px bg-black/15" />
      <button className={btn} onClick={() => s.setPlaying(!s.playing)} title="Play / pause (space)">
        {s.playing ? '❚❚' : '▶'}
      </button>
      {[1, 5, 10, 30].map((v) => (
        <button key={v} className={`${btn} ${s.speed === v ? 'bg-black text-white hover:bg-black' : ''}`} onClick={() => s.setSpeed(v)}>
          {v}×
        </button>
      ))}
      <span className="mx-1 h-5 w-px bg-black/15" />
      <select
        className="rounded-full bg-black/5 px-2 py-1 outline-none"
        value=""
        onChange={(e) => e.target.value && s.jump(Number(e.target.value))}
      >
        <option value="">Jump to…</option>
        {JUMPS.map(([label, t]) => (
          <option key={label} value={t}>
            {label} ({fmtClock(t)})
          </option>
        ))}
      </select>
      <span className="ml-1 w-14 text-right font-mono text-xs text-black/60">{fmtClock(s.now)}</span>
    </div>
  )
}
