// PROTOTYPE, throwaway. Variant B, "Big screen": a strip of huge numbers
// across the top for reading from a distance, one full-width chart with gas
// and air folded into its bottom band, and one big "next event" button.
// Clicking the chart opens a note dialog stamped at that moment (now), the
// way the roaster works in Cropster today.
import { useEffect, useRef, useState } from 'react'
import type uPlot from 'uplot'
import { C, Chart, baseOptions, clockAxis, fade, line, stepped, valueAxis } from '../chart/Chart'
import { eventLines, noteDots, nowLine, phaseBands, refTicks } from '../chart/draw'
import { EVENT_HINT, EVENT_KEY, EVENT_LABEL, EVENT_SHORT, MARKABLE, X_MAX, X_MIN, fmt1, fmtClock, fmtDelta, liveAt, refAt } from '../lib/roast'
import { columns, type Session } from '../lib/session'
import { Button, Kbd, Label, NextRoastFields, RoastedWeightField, StatusChip, cn, useHotkeys } from '../ui'

export function VariantB({ s }: { s: Session }) {
  const [dialogAt, setDialogAt] = useState<number | null>(null)
  useHotkeys(s, () => s.status !== 'waiting' && setDialogAt(s.now))
  const c = columns(s)
  return (
    <div className="flex h-screen flex-col">
      <div className="flex h-10 shrink-0 items-center gap-3 border-b border-line px-5 text-sm">
        <StatusChip s={s} />
        <span className="text-white/80">{s.next.coffee}</span>
        <span className="font-mono text-white/45">{s.next.greenKg} kg</span>
        <span className="ml-auto text-white/45">{s.ref ? `vs ${s.ref.label} · ${s.ref.date}` : 'No reference'}</span>
      </div>

      {s.status === 'done' ? <SummaryStrip s={s} /> : <NumberStrip s={s} />}

      <div className="relative min-h-0 flex-1">
        <Chart
          className="absolute inset-0 px-2 pt-2"
          options={options}
          data={[c.x, c.bt, c.et, c.ror, c.refBt, c.refEt, c.refRor, c.gas, c.fan]}
          overlay={s}
          draw={draw}
          onClick={() => s.status !== 'waiting' && setDialogAt(s.now)}
        />
        {s.status === 'waiting' && (
          <div className="absolute top-6 right-8 w-[min(420px,40%)] rounded-2xl border border-line bg-panel/95 p-5 shadow-2xl backdrop-blur">
            <div className="mb-4 flex items-baseline justify-between">
              <span className="text-lg font-semibold">Next roast</span>
              <span className="text-xs text-white/40">attached at Charge</span>
            </div>
            <NextRoastFields s={s} compact />
          </div>
        )}
      </div>

      <footer className="flex shrink-0 items-center gap-6 border-t border-line px-5 py-3 pb-16">
        <PhaseBar s={s} />
        {s.status === 'done' ? (
          <RoastedWeightField s={s} />
        ) : (
          <div className="flex items-center gap-2">
            {s.status === 'recording' && (
              <>
                <Button variant="ghost" onClick={() => setDialogAt(s.now)}>
                  Note <Kbd>N</Kbd>
                </Button>
                {MARKABLE.filter((k) => k !== s.nextEvent && !s.events.some((e) => e.kind === k)).map((k) => (
                  <Button key={k} variant="ghost" onClick={() => s.mark(k)}>
                    {EVENT_LABEL[k]} <Kbd>{EVENT_KEY[k]}</Kbd>
                  </Button>
                ))}
              </>
            )}
            {s.nextEvent && <NextButton s={s} />}
          </div>
        )}
      </footer>

      {dialogAt !== null && <NoteDialog s={s} at={dialogAt} onClose={() => setDialogAt(null)} />}
    </div>
  )
}

function Big({ label, value, unit, color, sub, dim }: { label: string; value: string; unit?: string; color?: string; sub?: string; dim?: boolean }) {
  return (
    <div className={cn('flex min-w-0 flex-col justify-center border-r border-line px-6 py-3 last:border-r-0', dim && 'opacity-40')}>
      <Label>{label}</Label>
      <div className="flex items-baseline gap-2">
        <span className="tnum font-mono text-[clamp(2.25rem,4.6vw,9rem)] leading-[0.95] font-bold" style={{ color }}>
          {value}
        </span>
        {unit && <span className="text-[clamp(0.8rem,1.1vw,1.5rem)] text-white/40">{unit}</span>}
      </div>
      <span className="tnum h-5 font-mono text-[clamp(0.75rem,0.9vw,1.1rem)] text-white/45">{sub}</span>
    </div>
  )
}

const vsRef = (cur: number | undefined, ref: number | undefined) =>
  ref === undefined || cur === undefined ? '' : `${fmtDelta(cur - ref)} vs ref`

export function NumberStrip({ s }: { s: Session }) {
  const l = s.live
  const waiting = s.status === 'waiting'
  return (
    <div className="grid shrink-0 grid-cols-[1.1fr_1.15fr_1fr_1.15fr_0.8fr] border-b border-line bg-panel">
      <Big label={waiting ? 'Waiting for charge' : 'Roast time'} value={waiting ? '–:––' : fmtClock(s.now)} dim={waiting} />
      <Big label="BT" value={fmt1(l.bt)} unit="°C" color={C.bt} sub={vsRef(l.bt, l.refBt)} />
      <Big label="RoR" value={fmt1(l.ror)} unit="°/min" color={C.ror} sub={vsRef(l.ror, l.refRor)} dim={l.ror === undefined} />
      <Big label="ET" value={fmt1(l.et)} unit="°C" color={C.et} sub={vsRef(l.et, l.refEt)} />
      <Big label="Air" value={l.fan?.toFixed(0) ?? '—'} unit="%" color={C.air} sub={`gas ${l.gas?.toFixed(0) ?? '—'} %`} />
    </div>
  )
}

export function SummaryStrip({ s }: { s: Session }) {
  const drop = s.events.find((e) => e.kind === 'drop')
  const fc = s.events.find((e) => e.kind === 'first_crack')
  return (
    <div className="grid shrink-0 grid-cols-5 border-b border-line bg-panel">
      <Big label="Total time" value={fmtClock(s.roastEnd)} />
      <Big label="Development time ratio" value={s.devRatio !== undefined ? s.devRatio.toFixed(1) : '—'} unit="%" color={C.ror} />
      <Big label="Development" value={fc ? fmtClock(s.roastEnd - fc.t) : '—'} />
      <Big label="Drop BT" value={fmt1(drop ? liveAt(drop.t)?.[1] : undefined)} unit="°C" color={C.bt} />
      <Big label="Weight loss" value={s.weightLoss !== undefined ? s.weightLoss.toFixed(1) : '—'} unit="%" />
    </div>
  )
}

function options() {
  return {
    ...baseOptions,
    scales: {
      x: { time: false, range: [X_MIN, X_MAX] as uPlot.Range.MinMax },
      temp: { range: [50, 250] as uPlot.Range.MinMax },
      ror: { range: [0, 30] as uPlot.Range.MinMax },
      pct: { range: [0, 420] as uPlot.Range.MinMax },
    },
    axes: [clockAxis(), valueAxis('temp', { values: (_: uPlot, v: number[]) => v.map((x) => `${x}°`) }), valueAxis('ror', { side: 1, grid: { show: false } })],
    series: [
      {},
      line('BT', 'temp', C.bt, { width: 3.5 }),
      line('ET', 'temp', C.et, { width: 2.5 }),
      line('RoR', 'ror', C.ror, { width: 2 }),
      line('Ref BT', 'temp', fade(C.bt, 0.35), { dash: [8, 6], width: 2.5 }),
      line('Ref ET', 'temp', fade(C.et, 0.3), { dash: [8, 6], width: 2 }),
      line('Ref RoR', 'ror', fade(C.ror, 0.3), { dash: [8, 6], width: 1.5 }),
      line('Gas', 'pct', fade(C.gas, 0.8), { paths: stepped, width: 1.5, fill: fade(C.gas, 0.08) }),
      line('Air', 'pct', fade(C.air, 0.8), { paths: stepped, width: 1.5 }),
    ],
  }
}

function draw(u: uPlot, s: Session) {
  phaseBands(u, s.phases)
  refTicks(u, s.refEvents.filter((e) => e.kind !== 'charge').map((e) => ({ t: e.t, label: `ref ${EVENT_SHORT[e.kind]}` })))
  if (s.status === 'waiting') nowLine(u, 0, 'now')
  eventLines(u, s.events.map((e) => ({ t: e.t, label: `${EVENT_LABEL[e.kind]} ${fmtClock(e.t)}`, strong: true })))
  if (s.ref) noteDots(u, s.refNotes.map((n, i) => ({ t: n.t, v: refAt(s.ref!, n.t)?.[1] ?? 150, label: String(i + 1), ghost: true })), 'temp')
  noteDots(u, s.notes.map((n, i) => ({ t: n.t, v: liveAt(n.t)?.[1] ?? 150, label: String(i + 1) })), 'temp')
}

function PhaseBar({ s }: { s: Session }) {
  const total = Math.max(1, s.roastEnd)
  return (
    <div className="grid min-w-0 flex-1 gap-1.5">
      <div className="flex h-9 overflow-hidden rounded-lg bg-white/5 text-xs font-medium">
        {s.phases.map((p) => {
          const pct = ((p.to - p.from) / total) * 100
          return (
            <div
              key={p.key}
              className={cn(
                'flex items-center justify-between gap-2 overflow-hidden px-3 whitespace-nowrap',
                p.key === 'drying' && 'bg-et/25',
                p.key === 'maillard' && 'bg-bt/25',
                p.key === 'development' && 'bg-ror/30',
              )}
              style={{ width: `${pct}%` }}
            >
              <span>{p.label}</span>
              <span className="tnum font-mono text-white/70">
                {fmtClock(p.to - p.from)} · {pct.toFixed(0)}%
              </span>
            </div>
          )
        })}
        {s.phases.length === 0 && <span className="self-center px-3 text-white/30">Phases appear after Charge</span>}
      </div>
    </div>
  )
}

function NextButton({ s }: { s: Session }) {
  const k = s.nextEvent!
  return (
    <Button variant={k === 'drop' ? 'outline' : 'primary'} className="h-14 min-w-72 px-6 text-lg" onClick={() => s.mark(k)}>
      {k === 'charge' || k === 'drop' ? EVENT_LABEL[k] : `Mark ${EVENT_LABEL[k]}`}
      {EVENT_HINT[k] && <span className="text-sm opacity-60">({EVENT_HINT[k]})</span>}
      <Kbd className={k === 'drop' ? '' : 'border-black/30 text-black/60'}>{EVENT_KEY[k]}</Kbd>
    </Button>
  )
}

function NoteDialog({ s, at, onClose }: { s: Session; at: number; onClose: () => void }) {
  const [text, setText] = useState('')
  const ta = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    ta.current?.focus()
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])
  const save = () => {
    s.addNote(text, at)
    onClose()
  }
  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/60" onClick={onClose}>
      <div className="grid w-[min(560px,90vw)] gap-4 rounded-2xl border border-white/10 bg-panel-2 p-6 shadow-2xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-baseline justify-between">
          <span className="text-xl font-semibold">
            Note at <span className="tnum font-mono">{fmtClock(at)}</span>
          </span>
          <span className="tnum font-mono text-sm text-bt">BT {fmt1(liveAt(at)?.[1])}°</span>
        </div>
        <textarea
          ref={ta}
          rows={3}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              save()
            }
          }}
          placeholder="Gas to 70, smell of bread…"
          className="w-full resize-none rounded-lg border border-line bg-bg px-3 py-2 text-base outline-none focus:border-accent/60"
        />
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs text-white/40">or mark at {fmtClock(at)}:</span>
          {MARKABLE.map((k) => (
            <Button
              key={k}
              variant="subtle"
              className="h-8 text-xs"
              onClick={() => {
                s.mark(k, at)
                onClose()
              }}
            >
              {EVENT_LABEL[k]}
            </Button>
          ))}
          <Button variant="primary" className="ml-auto" onClick={save}>
            Save note
          </Button>
        </div>
      </div>
    </div>
  )
}
