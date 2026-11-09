// PROTOTYPE, throwaway. Variant C, "Logbook": a timeline of everything that
// happened on the left (events, notes, what the reference did next), three
// stacked charts on the right (temperatures, RoR, gas/air) with the live
// values tagged on the head of each line. Clicking a chart pins an instant;
// the timeline's composer then marks an event or writes a note there.
import { useEffect, useRef, useState } from 'react'
import type uPlot from 'uplot'
import { C, Chart, baseOptions, clockAxis, fade, line, stepped, valueAxis } from '../chart/Chart'
import { eventLines, headTags, noteDots, nowLine, pin, refTicks, type Tag } from '../chart/draw'
import { EVENT_HINT, EVENT_KEY, EVENT_LABEL, EVENT_SHORT, MARKABLE, X_MAX, X_MIN, fmt1, fmtClock, fmtDelta, liveAt } from '../lib/roast'
import { columns, type Session } from '../lib/session'
import { Button, Kbd, Label, NextRoastFields, RoastedWeightField, StatusChip, cn, useHotkeys } from '../ui'

const sync = { key: 'C' }
const x = { time: false, range: [X_MIN, X_MAX] as uPlot.Range.MinMax }
// Empty right axis: room for the value tags and the same plot width in all three charts.
const gutter = (scale: string) => valueAxis(scale, { side: 1, size: 110, grid: { show: false }, values: () => [] })

export function VariantC({ s }: { s: Session }) {
  const [pinned, setPinned] = useState<number | null>(null)
  const noteInput = useRef<HTMLInputElement>(null)
  useHotkeys(s, () => noteInput.current?.focus(), pinned ?? undefined)
  const c = columns(s)
  const headT = s.status === 'waiting' ? 0 : s.now
  const overlay = { s, pinned, headT }
  const onClick = (k: { t: number }) => {
    if (s.status === 'waiting') return
    setPinned(Math.max(0, Math.min(k.t, s.now)))
    noteInput.current?.focus()
  }

  return (
    <div className="grid h-screen grid-cols-[clamp(300px,23vw,400px)_1fr]">
      <aside className="flex min-h-0 flex-col border-r border-line bg-panel">
        <div className="grid gap-3 border-b border-line p-4">
          <div className="flex items-center justify-between">
            <span className="font-mono text-sm font-bold tracking-widest text-accent">ROASTRO</span>
            <StatusChip s={s} />
          </div>
          {s.status === 'waiting' ? (
            <NextRoastFields s={s} />
          ) : (
            <div className="grid gap-0.5 text-sm">
              <span className="text-white/90">{s.next.coffee}</span>
              <span className="font-mono text-xs text-white/45">
                {s.next.greenKg} kg · {s.next.roastedBy} · {s.ref ? `ref ${s.ref.label} (${s.ref.date})` : 'no reference'}
              </span>
            </div>
          )}
        </div>

        <Timeline s={s} pinned={pinned} />

        {s.status !== 'waiting' && (
          <Composer s={s} pinned={pinned} setPinned={setPinned} input={noteInput} />
        )}
        {s.status === 'done' && (
          <div className="border-t border-line p-4">
            <RoastedWeightField s={s} />
          </div>
        )}
        {s.status === 'waiting' && (
          <div className="mt-auto grid gap-2 border-t border-line p-4">
            <Button variant="primary" className="py-3" onClick={() => s.mark('charge')}>
              Charge <Kbd className="border-black/30 text-black/60">C</Kbd>
            </Button>
            <span className="text-xs text-white/40">Detected automatically; press only if it misses.</span>
          </div>
        )}
      </aside>

      <main className="flex min-h-0 min-w-0 flex-col pb-14">
        <Header s={s} />
        <div className="relative min-h-0 flex-[5]">
          <ChartLabel>Temperature</ChartLabel>
          <Chart className="absolute inset-0 pl-2" options={tempOptions} data={[c.x, c.bt, c.et, c.refBt, c.refEt]} overlay={overlay} draw={drawTemp} onClick={onClick} />
        </div>
        <div className="relative min-h-0 flex-[2.4] border-t border-line">
          <ChartLabel>Rate of rise °C/min</ChartLabel>
          <Chart className="absolute inset-0 pl-2" options={rorOptions} data={[c.x, c.ror, c.refRor]} overlay={overlay} draw={drawRor} onClick={onClick} />
        </div>
        <div className="relative min-h-0 flex-[1.6] border-t border-line">
          <ChartLabel>Gas · Air %</ChartLabel>
          <Chart className="absolute inset-0 pl-2" options={pctOptions} data={[c.x, c.gas, c.fan]} overlay={overlay} draw={drawPct} onClick={onClick} />
        </div>
      </main>
    </div>
  )
}

function ChartLabel({ children }: { children: string }) {
  return <span className="pointer-events-none absolute top-2 left-16 z-10 text-[11px] tracking-wider text-white/35 uppercase">{children}</span>
}

function Header({ s }: { s: Session }) {
  const phase = s.phases.find((p) => p.open) ?? s.phases[s.phases.length - 1]
  return (
    <div className="flex shrink-0 items-end gap-8 border-b border-line px-6 py-3">
      <div className="flex flex-col">
        <Label>{s.status === 'waiting' ? 'Waiting for charge' : s.status === 'done' ? 'Total time' : 'Roast time'}</Label>
        <span className="tnum font-mono text-[clamp(2.5rem,4.5vw,5.5rem)] leading-none font-bold">
          {s.status === 'waiting' ? '–:––' : fmtClock(s.status === 'done' ? s.roastEnd : s.now)}
        </span>
      </div>
      {phase && (
        <div className="flex flex-col pb-1">
          <Label>Phase</Label>
          <span className="text-2xl font-semibold">
            {phase.label} <span className="tnum font-mono text-lg text-white/50">{fmtClock(phase.to - phase.from)}</span>
          </span>
        </div>
      )}
      <div className="flex flex-col pb-1">
        <Label>Development time ratio</Label>
        <span className="tnum font-mono text-2xl font-bold text-ror">{s.devRatio !== undefined ? `${s.devRatio.toFixed(1)}%` : '—'}</span>
      </div>
      {s.live.refBt !== undefined && s.live.bt !== undefined && (
        <div className="flex flex-col pb-1">
          <Label>BT vs reference</Label>
          <span className="tnum font-mono text-2xl font-bold">{fmtDelta(s.live.bt - s.live.refBt)}°</span>
        </div>
      )}
    </div>
  )
}

type Overlay = { s: Session; pinned: number | null; headT: number }

const marks = (s: Session, pills: boolean) =>
  s.events.map((e) => ({ t: e.t, label: pills ? `${EVENT_SHORT[e.kind]} ${fmtClock(e.t)}` : '', strong: true }))

function common(u: uPlot, o: Overlay, pills: boolean) {
  if (o.s.status === 'waiting') nowLine(u, 0, pills ? 'now' : undefined)
  eventLines(u, marks(o.s, pills), { pills })
  if (o.pinned !== null) pin(u, o.pinned, fmtClock(o.pinned))
}

function tempOptions() {
  return {
    ...baseOptions,
    cursor: { ...baseOptions.cursor, sync },
    scales: { x, temp: { range: [50, 250] as uPlot.Range.MinMax } },
    axes: [clockAxis(false), valueAxis('temp', { values: (_: uPlot, v: number[]) => v.map((x) => `${x}°`) }), gutter('temp')],
    series: [
      {},
      line('BT', 'temp', C.bt, { width: 2.5 }),
      line('ET', 'temp', C.et),
      line('Ref BT', 'temp', fade(C.bt, 0.35), { dash: [6, 5] }),
      line('Ref ET', 'temp', fade(C.et, 0.3), { dash: [6, 5], width: 1.5 }),
    ],
  }
}

function drawTemp(u: uPlot, o: Overlay) {
  refTicks(u, o.s.refEvents.filter((e) => e.kind !== 'charge').map((e) => ({ t: e.t, label: `ref ${EVENT_SHORT[e.kind]}` })))
  common(u, o, true)
  noteDots(u, o.s.notes.map((n, i) => ({ t: n.t, v: liveAt(n.t)?.[1] ?? 150, label: String(i + 1) })), 'temp')
  const l = o.s.live
  const tags: Tag[] = []
  if (l.bt !== undefined) tags.push({ t: o.headT, v: l.bt, scale: 'temp', color: C.bt, text: `BT ${fmt1(l.bt)}` })
  if (l.et !== undefined) tags.push({ t: o.headT, v: l.et, scale: 'temp', color: C.et, text: `ET ${fmt1(l.et)}` })
  headTags(u, tags)
}

function rorOptions() {
  return {
    ...baseOptions,
    cursor: { ...baseOptions.cursor, sync },
    scales: { x, ror: { range: [0, 30] as uPlot.Range.MinMax } },
    axes: [clockAxis(false), valueAxis('ror', { splits: () => [0, 10, 20, 30] }), gutter('ror')],
    series: [{}, line('RoR', 'ror', C.ror, { width: 2 }), line('Ref RoR', 'ror', fade(C.ror, 0.35), { dash: [6, 5] })],
  }
}

function drawRor(u: uPlot, o: Overlay) {
  common(u, o, false)
  const l = o.s.live
  if (l.ror !== undefined) headTags(u, [{ t: o.headT, v: l.ror, scale: 'ror', color: C.ror, text: `${fmt1(l.ror)}${l.refRor !== undefined ? `  ${fmtDelta(l.ror - l.refRor)}` : ''}` }])
}

function pctOptions() {
  return {
    ...baseOptions,
    cursor: { ...baseOptions.cursor, sync },
    scales: { x, pct: { range: [0, 100] as uPlot.Range.MinMax } },
    axes: [clockAxis(), valueAxis('pct', { splits: () => [0, 50, 100] }), gutter('pct')],
    series: [{}, line('Gas', 'pct', C.gas, { paths: stepped }), line('Air', 'pct', C.air, { paths: stepped })],
  }
}

function drawPct(u: uPlot, o: Overlay) {
  common(u, o, false)
  const l = o.s.live
  const tags: Tag[] = []
  if (l.gas !== undefined) tags.push({ t: o.headT, v: l.gas, scale: 'pct', color: C.gas, text: `gas ${l.gas.toFixed(0)}` })
  if (l.fan !== undefined) tags.push({ t: o.headT, v: l.fan, scale: 'pct', color: C.air, text: `air ${l.fan.toFixed(0)}` })
  headTags(u, tags)
}

type Entry =
  | { type: 'event'; t: number; label: string; source: string; kind: string; removable: boolean }
  | { type: 'note'; t: number; text: string; id: number; n: number }

function Timeline({ s, pinned }: { s: Session; pinned: number | null }) {
  const end = useRef<HTMLDivElement>(null)
  const entries: Entry[] = [
    ...s.events.map((e) => ({
      type: 'event' as const,
      t: e.t,
      label: EVENT_LABEL[e.kind],
      source: e.source,
      kind: e.kind,
      removable: MARKABLE.includes(e.kind),
    })),
    ...s.notes.map((n, i) => ({ type: 'note' as const, t: n.t, text: n.text, id: n.id, n: i + 1 })),
  ].sort((a, b) => a.t - b.t)
  // What the reference roast did next, as a guide.
  const upcoming: { t: number; label: string }[] =
    s.status === 'done'
      ? []
      : s.refEvents
          .filter((e) => e.kind !== 'charge' && !s.events.some((x) => x.kind === e.kind) && e.t > Math.max(s.now, 0))
          .map((e) => ({ t: e.t, label: EVENT_LABEL[e.kind] }))

  useEffect(() => {
    end.current?.scrollIntoView({ block: 'nearest' })
  }, [entries.length])

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
      <Label>Timeline</Label>
      <ol className="relative mt-3 grid gap-0.5 border-l border-white/10 pl-4">
        {entries.length === 0 && <li className="py-2 text-sm text-white/35">Events and notes will appear here.</li>}
        {entries.map((e) => (
          <li key={e.type === 'note' ? `n${e.id}` : `e${e.t}${e.type === 'event' ? e.kind : ''}`} className="group relative flex items-baseline gap-3 py-1.5 text-sm">
            <span className={cn('absolute top-3 -left-[21px] size-2.5 rounded-full', e.type === 'note' ? 'bg-white' : 'bg-accent')} />
            <span className="tnum w-11 shrink-0 font-mono text-xs text-white/50">{fmtClock(e.t)}</span>
            {e.type === 'event' && (
              <>
                <span className="font-semibold">{e.label}</span>
                <span className="tnum font-mono text-xs text-bt">{fmt1(liveAt(e.t)?.[1])}°</span>
                <span className="text-[10px] text-white/30 uppercase">{e.source}</span>
                {e.removable && (
                  <button className="ml-auto text-white/0 group-hover:text-white/40 hover:!text-white" onClick={() => s.unmark(e.kind as never)}>
                    ✕
                  </button>
                )}
              </>
            )}
            {e.type === 'note' && (
              <>
                <span className="flex-1 text-white/85">
                  <span className="mr-1.5 text-white/40">{e.n}.</span>
                  {e.text}
                </span>
                <button className="text-white/0 group-hover:text-white/40 hover:!text-white" onClick={() => s.removeNote(e.id)}>
                  ✕
                </button>
              </>
            )}
          </li>
        ))}
        {s.status !== 'waiting' && s.status !== 'done' && (
          <li className="relative flex items-baseline gap-3 py-1.5 text-sm">
            <span className="absolute top-3 -left-[22px] size-3 animate-pulse rounded-full border-2 border-accent bg-bg" />
            <span className="tnum w-11 font-mono text-xs text-accent">{fmtClock(s.now)}</span>
            <span className="text-accent">now{pinned !== null ? ` · pinned ${fmtClock(pinned)}` : ''}</span>
          </li>
        )}
        {upcoming.map((e) => (
          <li key={`u${e.label}`} className="relative flex items-baseline gap-3 py-1.5 text-sm text-white/30">
            <span className="absolute top-3 -left-[21px] size-2.5 rounded-full border border-white/25 bg-bg" />
            <span className="tnum w-11 font-mono text-xs">{fmtClock(e.t)}</span>
            <span>{e.label}</span>
            <span className="text-[10px] uppercase">in reference</span>
          </li>
        ))}
      </ol>
      <div ref={end} />
    </div>
  )
}

function Composer({
  s,
  pinned,
  setPinned,
  input,
}: {
  s: Session
  pinned: number | null
  setPinned: (t: number | null) => void
  input: React.RefObject<HTMLInputElement | null>
}) {
  const [text, setText] = useState('')
  const at = pinned ?? undefined
  return (
    <div className="grid gap-2 border-t border-line bg-panel-2/60 p-4">
      <div className="flex items-baseline justify-between">
        <span className="text-xs text-white/50">
          {pinned !== null ? (
            <>
              At <span className="tnum font-mono text-white">{fmtClock(pinned)}</span> (pinned)
            </>
          ) : (
            <>At <span className="text-white">now</span> · click a chart to pin another instant</>
          )}
        </span>
        {pinned !== null && (
          <button className="text-xs text-white/40 hover:text-white" onClick={() => setPinned(null)}>
            unpin
          </button>
        )}
      </div>
      <div className="grid grid-cols-3 gap-1.5">
        {MARKABLE.map((k) => {
          const done = s.events.some((e) => e.kind === k)
          return (
            <Button
              key={k}
              variant={s.nextEvent === k ? 'primary' : 'subtle'}
              className="h-auto flex-col gap-0.5 py-2 text-xs"
              onClick={() => {
                s.mark(k, at)
                setPinned(null)
              }}
            >
              <span>
                {EVENT_LABEL[k]} <Kbd className={s.nextEvent === k ? 'border-black/30 text-black/60' : ''}>{EVENT_KEY[k]}</Kbd>
              </span>
              <span className="text-[10px] opacity-60">{done ? 'move here' : (EVENT_HINT[k] ?? ' ')}</span>
            </Button>
          )
        })}
      </div>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          s.addNote(text, at)
          setText('')
          setPinned(null)
          input.current?.blur()
        }}
      >
        <input
          ref={input}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === 'Escape' && (setPinned(null), e.currentTarget.blur())}
          placeholder="Note… (N, Enter to save)"
          className="min-w-0 flex-1 rounded-lg border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-accent/60"
        />
        {s.status === 'recording' && (
          <Button variant="outline" type="button" onClick={() => s.mark('drop')}>
            Drop <Kbd>X</Kbd>
          </Button>
        )}
      </form>
    </div>
  )
}
