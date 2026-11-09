// PROTOTYPE, throwaway. Variant A, "Cockpit": the chart dominates, a rail of
// big numbers on the right, gas and air in a strip under the chart. Clicking
// the chart opens a menu right where you clicked and stamps that instant.
import { useEffect, useRef, useState } from 'react'
import type uPlot from 'uplot'
import { C, Chart, baseOptions, clockAxis, fade, line, stepped, valueAxis, type ChartClick } from '../chart/Chart'
import { eventLines, noteDots, nowLine, phaseBands, refTicks } from '../chart/draw'
import {
  EVENT_HINT,
  EVENT_KEY,
  EVENT_LABEL,
  EVENT_ORDER,
  EVENT_SHORT,
  MARKABLE,
  X_MAX,
  X_MIN,
  fmt1,
  fmtClock,
  fmtDelta,
  liveAt,
  refAt,
} from '../lib/roast'
import { columns, type Session } from '../lib/session'
import { Button, Kbd, Label, NextRoastFields, RoastedWeightField, StatusChip, cn, useHotkeys } from '../ui'

const sync = { key: 'A' }

export function VariantA({ s }: { s: Session }) {
  const [menu, setMenu] = useState<ChartClick | null>(null)
  useHotkeys(s, () => setMenu({ t: s.now, value: null, clientX: innerWidth * 0.5, clientY: innerHeight * 0.4 }))
  const c = columns(s)

  const overlay = {
    s,
    marks: s.events.map((e) => ({ t: e.t, label: `${EVENT_SHORT[e.kind]} ${fmtClock(e.t)}`, strong: true })),
  }

  return (
    <div className="flex h-screen flex-col">
      <header className="flex h-14 shrink-0 items-center gap-4 border-b border-line px-5">
        <span className="font-mono text-sm font-bold tracking-widest text-accent">ROASTRO</span>
        <StatusChip s={s} />
        <span className="truncate text-sm text-white/85">{s.next.coffee}</span>
        <span className="font-mono text-sm text-white/50">{s.next.greenKg} kg</span>
        <span className="ml-auto text-sm text-white/50">
          {s.ref ? (
            <>
              Reference <span className="text-white/80">{s.ref.label}</span> · {s.ref.date}
            </>
          ) : (
            'No reference'
          )}
        </span>
      </header>

      <div className="grid min-h-0 flex-1 grid-cols-[1fr_clamp(300px,24vw,440px)]">
        <main className="flex min-h-0 min-w-0 flex-col overflow-hidden pb-16">
          <Legend s={s} />
          <Chart
            className="min-h-0 flex-1 px-2"
            options={mainOptions}
            data={[c.x, c.bt, c.et, c.ror, c.refBt, c.refEt, c.refRor]}
            overlay={overlay}
            draw={drawMain}
            onClick={(k) => s.status !== 'waiting' && setMenu(k)}
          />
          <div className="flex items-center gap-4 px-6 pt-2 text-[11px] text-white/45">
            <span className="text-gas">■ Gas %</span>
            <span className="text-air">■ Air %</span>
          </div>
          <Chart className="h-[16%] min-h-24 px-2" options={stripOptions} data={[c.x, c.gas, c.fan]} overlay={overlay} draw={drawStrip} />
        </main>

        <aside className="flex min-h-0 flex-col gap-5 overflow-y-auto border-l border-line bg-panel p-5 pb-20">
          {s.status === 'waiting' ? <Waiting s={s} /> : <Readouts s={s} />}
          {s.status !== 'waiting' && <Phases s={s} />}
          {s.status !== 'waiting' && <Events s={s} />}
          {s.status === 'done' && <RoastedWeightField s={s} />}
        </aside>
      </div>

      {menu && <ClickMenu s={s} at={menu} onClose={() => setMenu(null)} />}
    </div>
  )
}

export function Legend({ s }: { s: Session }) {
  return (
    <div className="flex items-center gap-5 px-6 pt-3 text-[11px] text-white/50">
      <span className="text-bt">━ BT</span>
      <span className="text-et">━ ET</span>
      <span className="text-ror">━ RoR (right axis)</span>
      {s.ref && <span className="text-white/40">┅ {s.ref.label}</span>}
      <span className="ml-auto text-white/35">Click the chart to mark an event or add a note</span>
    </div>
  )
}

export function mainOptions() {
  return {
    ...baseOptions,
    cursor: { ...baseOptions.cursor, sync },
    scales: { x: { time: false, range: [X_MIN, X_MAX] as uPlot.Range.MinMax }, temp: { range: [50, 250] as uPlot.Range.MinMax }, ror: { range: [0, 30] as uPlot.Range.MinMax } },
    axes: [clockAxis(), valueAxis('temp', { values: (_: uPlot, v: number[]) => v.map((x) => `${x}°`) }), valueAxis('ror', { side: 1, grid: { show: false } })],
    series: [
      {},
      line('BT', 'temp', C.bt, { width: 2.5 }),
      line('ET', 'temp', C.et),
      line('RoR', 'ror', C.ror, { width: 1.5 }),
      line('Ref BT', 'temp', fade(C.bt, 0.35), { dash: [6, 5], width: 2 }),
      line('Ref ET', 'temp', fade(C.et, 0.3), { dash: [6, 5], width: 1.5 }),
      line('Ref RoR', 'ror', fade(C.ror, 0.3), { dash: [6, 5], width: 1.25 }),
    ],
  }
}

export function drawMain(u: uPlot, o: { s: Session; marks: { t: number; label: string }[] }) {
  const s = o.s
  phaseBands(u, s.phases)
  refTicks(u, s.refEvents.filter((e) => e.kind !== 'charge').map((e) => ({ t: e.t, label: EVENT_SHORT[e.kind] })))
  if (s.status === 'waiting') nowLine(u, 0, 'now')
  eventLines(u, o.marks)
  if (s.ref) noteDots(u, s.refNotes.map((n, i) => ({ t: n.t, v: refAt(s.ref!, n.t)?.[1] ?? 150, label: String(i + 1), ghost: true })), 'temp')
  noteDots(u, s.notes.map((n, i) => ({ t: n.t, v: liveAt(n.t)?.[1] ?? 150, label: String(i + 1) })), 'temp')
}

export function stripOptions() {
  return {
    ...baseOptions,
    cursor: { ...baseOptions.cursor, sync },
    scales: { x: { time: false, range: [X_MIN, X_MAX] as uPlot.Range.MinMax }, pct: { range: [0, 100] as uPlot.Range.MinMax } },
    axes: [
      clockAxis(false),
      valueAxis('pct', { values: (_: uPlot, v: number[]) => v.map((x) => `${x}%`), splits: () => [0, 50, 100] }),
      valueAxis('pct', { side: 1, grid: { show: false }, values: () => [] }),
    ],
    series: [{}, line('Gas', 'pct', C.gas, { paths: stepped }), line('Air', 'pct', C.air, { paths: stepped })],
  }
}

export function drawStrip(u: uPlot, o: { s: Session; marks: { t: number; label: string }[] }) {
  eventLines(u, o.marks, { pills: false })
  if (o.s.status === 'waiting') nowLine(u, 0)
}

export function Readout({ label, value, unit, color, sub, size = 'md' }: { label: string; value: string; unit?: string; color?: string; sub?: string; size?: 'xl' | 'lg' | 'md' }) {
  return (
    <div className="flex flex-col">
      <Label>{label}</Label>
      <div className="flex items-baseline gap-1.5">
        <span
          className={cn(
            'tnum font-mono leading-none font-bold',
            size === 'xl' && 'text-[clamp(2.5rem,4vw,5.5rem)]',
            size === 'lg' && 'text-[clamp(2rem,2.9vw,4.25rem)]',
            size === 'md' && 'text-[clamp(1.5rem,2vw,2.5rem)]',
          )}
          style={{ color }}
        >
          {value}
        </span>
        {unit && <span className="text-sm text-white/40">{unit}</span>}
      </div>
      {sub && <span className="tnum mt-1 font-mono text-xs text-white/45">{sub}</span>}
    </div>
  )
}

const refSub = (cur: number | undefined, ref: number | undefined) =>
  ref === undefined ? undefined : `ref ${fmt1(ref)}  ${cur !== undefined ? fmtDelta(cur - ref) : ''}`

export function Readouts({ s }: { s: Session }) {
  const l = s.live
  return (
    <div className="grid grid-cols-2 gap-x-4 gap-y-5">
      <div className="col-span-2">
        <Readout label="Roast time" value={fmtClock(s.status === 'done' ? s.roastEnd : s.now)} size="xl" />
      </div>
      <div className="col-span-2">
        <Readout label="Bean temp · BT" value={fmt1(l.bt)} unit="°C" color={C.bt} size="lg" sub={refSub(l.bt, l.refBt)} />
      </div>
      <Readout label="RoR" value={fmt1(l.ror)} unit="°/min" color={C.ror} sub={refSub(l.ror, l.refRor)} />
      <Readout label="Air temp · ET" value={fmt1(l.et)} unit="°C" color={C.et} sub={refSub(l.et, l.refEt)} />
      <Readout label="Air" value={l.fan?.toFixed(0) ?? '—'} unit="%" color={C.air} />
      <Readout label="Gas" value={l.gas?.toFixed(0) ?? '—'} unit="%" color={C.gas} />
    </div>
  )
}

function Waiting({ s }: { s: Session }) {
  return (
    <>
      <div className="grid gap-4 rounded-xl border border-line bg-panel-2/50 p-4">
        <span className="text-sm font-semibold">Next roast</span>
        <NextRoastFields s={s} />
      </div>
      <div className="grid grid-cols-2 gap-4">
        <Readout label="BT" value={fmt1(s.live.bt)} unit="°C" color={C.bt} size="lg" />
        <Readout label="ET" value={fmt1(s.live.et)} unit="°C" color={C.et} size="lg" />
      </div>
      <Button variant="primary" className="py-3 text-base" onClick={() => s.mark('charge')}>
        Charge <Kbd className="border-black/30 text-black/60">C</Kbd>
      </Button>
      <p className="text-xs text-white/40">Charge is detected automatically; press it only if detection misses.</p>
    </>
  )
}

export function Phases({ s }: { s: Session }) {
  const total = Math.max(1, s.roastEnd)
  return (
    <div className="grid gap-2">
      <div className="flex items-baseline justify-between">
        <Label>Phases</Label>
        <span className="font-mono text-xs text-white/60">
          DTR <span className="text-base font-bold text-white">{s.devRatio !== undefined ? `${s.devRatio.toFixed(1)}%` : '—'}</span>
        </span>
      </div>
      <div className="flex h-2 overflow-hidden rounded-full bg-white/5">
        {s.phases.map((p) => (
          <div
            key={p.key}
            className={cn(p.key === 'drying' && 'bg-et/60', p.key === 'maillard' && 'bg-bt/60', p.key === 'development' && 'bg-ror/70')}
            style={{ width: `${((p.to - p.from) / total) * 100}%` }}
          />
        ))}
      </div>
      <div className="grid grid-cols-3 gap-2 font-mono text-xs">
        {(['drying', 'maillard', 'development'] as const).map((k) => {
          const p = s.phases.find((x) => x.key === k)
          return (
            <div key={k} className={cn('flex flex-col', !p && 'opacity-30')}>
              <span className="font-sans text-[11px] text-white/50 capitalize">{k}</span>
              <span>{p ? fmtClock(p.to - p.from) : '—'}</span>
              <span className="text-white/40">{p ? `${(((p.to - p.from) / total) * 100).toFixed(0)}%` : ''}</span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

function Events({ s }: { s: Session }) {
  return (
    <div className="grid gap-1">
      <Label>Events</Label>
      {EVENT_ORDER.map((kind) => {
        const e = s.events.find((x) => x.kind === kind)
        const markable = MARKABLE.includes(kind)
        const next = s.nextEvent === kind
        return (
          <div key={kind} className={cn('flex h-9 items-center gap-2 rounded-lg px-2 text-sm', next && 'bg-accent/10')}>
            <span className={cn('w-28', e ? 'text-white' : 'text-white/50')}>{EVENT_LABEL[kind]}</span>
            {e ? (
              <>
                <span className="tnum font-mono">{fmtClock(e.t)}</span>
                <span className="text-[10px] text-white/35 uppercase">{e.source}</span>
                {markable && (
                  <button className="ml-auto text-white/30 hover:text-white" onClick={() => s.unmark(kind)} title="Remove">
                    ✕
                  </button>
                )}
              </>
            ) : kind === 'turning_point' ? (
              <span className="text-xs text-white/30">computed</span>
            ) : (markable || kind === 'drop') && s.status === 'recording' ? (
              <Button variant={next ? 'primary' : 'subtle'} className="ml-auto h-7 px-2.5 text-xs" onClick={() => s.mark(kind)}>
                {kind === 'drop' ? 'Drop now' : 'Mark now'} {EVENT_KEY[kind] && <Kbd>{EVENT_KEY[kind]}</Kbd>}
              </Button>
            ) : null}
          </div>
        )
      })}
      {s.notes.length > 0 && (
        <div className="mt-2 grid gap-1">
          <Label>Notes</Label>
          {s.notes.map((n, i) => (
            <div key={n.id} className="flex items-start gap-2 text-sm">
              <span className="grid size-5 shrink-0 place-items-center rounded-full bg-white text-[10px] font-bold text-black">{i + 1}</span>
              <span className="tnum font-mono text-white/50">{fmtClock(n.t)}</span>
              <span className="flex-1 text-white/85">{n.text}</span>
              <button className="text-white/30 hover:text-white" onClick={() => s.removeNote(n.id)}>
                ✕
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

export function ClickMenu({ s, at, onClose }: { s: Session; at: ChartClick; onClose: () => void }) {
  const [text, setText] = useState('')
  const input = useRef<HTMLInputElement>(null)
  const future = at.t >= s.now
  const t = Math.max(0, Math.min(at.t, s.now))
  const bt = liveAt(t)?.[1]
  useEffect(() => {
    input.current?.focus()
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])
  const left = Math.min(at.clientX + 12, innerWidth - 340)
  const top = Math.min(at.clientY + 12, innerHeight - 280)
  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} />
      <div className="fixed z-50 grid w-80 gap-2 rounded-xl border border-white/10 bg-panel-2 p-3 shadow-2xl shadow-black/60" style={{ left, top }}>
        <div className="flex items-baseline justify-between">
          <span className="tnum font-mono text-sm font-bold">{future ? `now · ${fmtClock(t)}` : fmtClock(t)}</span>
          <span className="tnum font-mono text-xs text-bt">BT {fmt1(bt)}°</span>
        </div>
        {MARKABLE.map((kind) => {
          const done = s.events.find((e) => e.kind === kind)
          return (
            <Button
              key={kind}
              variant="subtle"
              className="justify-start whitespace-nowrap"
              onClick={() => {
                s.mark(kind, t)
                onClose()
              }}
            >
              {EVENT_LABEL[kind]}
              {EVENT_HINT[kind] && <span className="text-xs text-white/40">{EVENT_HINT[kind]}</span>}
              {done && <span className="ml-auto text-[10px] text-white/40">was {fmtClock(done.t)}</span>}
            </Button>
          )
        })}
        <form
          onSubmit={(e) => {
            e.preventDefault()
            s.addNote(text, t)
            onClose()
          }}
        >
          <input
            ref={input}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Add a note… (Enter)"
            className="w-full rounded-lg border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-accent/60"
          />
        </form>
      </div>
    </>
  )
}
