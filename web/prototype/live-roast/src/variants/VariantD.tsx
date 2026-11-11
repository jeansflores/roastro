// PROTOTYPE, throwaway. Variant D, "Cockpit + timeline": Jean's pick after
// the first round. Variant A's chart, strip and click menu, with variant C's
// timeline in the rail instead of A's event list. One "Big numbers" toggle
// moves only the readouts into variant B's top strip; marking stays the same.
import { useEffect, useRef, useState } from 'react'
import { C, Chart, fade } from '../chart/Chart'
import { EVENT_HINT, EVENT_KEY, EVENT_LABEL, MARKABLE, fmt1, fmtClock, fmtDelta, liveAt, type EventKind } from '../lib/roast'
import { columns, type Columns, type Session } from '../lib/session'
import { Button, Kbd, Label, NextRoastFields, RoastedWeightField, StatusChip, cn, useHotkeys } from '../ui'
import { ClickMenu, Legend, Phases, Readout, Readouts, drawMain, drawStrip, mainOptions, stripOptions } from './VariantA'
import { NumberStrip, SummaryStrip } from './VariantB'
import type { ChartClick } from '../chart/Chart'

export function VariantD({ s }: { s: Session }) {
  const [menu, setMenu] = useState<ChartClick | null>(null)
  const [big, setBig] = useState(() => new URLSearchParams(location.search).get('big') === '1')
  useHotkeys(s, () => s.status !== 'waiting' && setMenu({ t: s.now, value: null, clientX: innerWidth * 0.4, clientY: innerHeight * 0.35 }))
  const c = columns(s)
  const overlay = { s, marks: s.events.map((e) => ({ t: e.t, label: `${EVENT_LABEL[e.kind]} ${fmtClock(e.t)}`, strong: true })) }

  const toggleBig = () => {
    const url = new URL(location.href)
    url.searchParams.set('big', big ? '0' : '1')
    history.replaceState(null, '', url)
    setBig(!big)
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
        <button
          onClick={toggleBig}
          className={cn(
            'flex items-center gap-2 rounded-full border px-3 py-1 text-xs font-medium transition-colors',
            big ? 'border-accent/50 bg-accent/15 text-accent' : 'border-line text-white/60 hover:text-white',
          )}
          title="Move the readouts to a strip of big numbers"
        >
          <span className={cn('relative h-3.5 w-6 rounded-full', big ? 'bg-accent' : 'bg-white/20')}>
            <span className={cn('absolute top-0.5 size-2.5 rounded-full bg-black transition-all', big ? 'left-3' : 'left-0.5')} />
          </span>
          Big numbers
        </button>
      </header>

      {big && (s.status === 'done' ? <SummaryStrip s={s} /> : <NumberStrip s={s} />)}

      <div className={cn('grid min-h-0 flex-1', big ? 'grid-cols-[1fr_clamp(280px,21vw,380px)]' : 'grid-cols-[1fr_clamp(300px,24vw,440px)]')}>
        <main className="flex min-h-0 min-w-0 flex-col overflow-hidden pb-16">
          <Legend s={s} />
          <Chart
            className="min-h-0 flex-1 px-2"
            options={mainWithPoints}
            data={[c.x, c.bt, c.et, c.ror, c.refBt, c.refEt, c.refRor]}
            overlay={overlay}
            draw={drawMain}
            onClick={(k) => s.status !== 'waiting' && setMenu(k)}
            tooltip={(i) => <HoverCard s={s} c={c} i={i} />}
          />
          <div className="flex items-center gap-4 px-6 pt-2 text-[11px] text-white/45">
            <span className="text-gas">■ Gas %</span>
            <span className="text-air">■ Air %</span>
          </div>
          <Chart
            className="h-[16%] min-h-24 px-2"
            options={stripWithPoints}
            data={[c.x, c.gas, c.fan]}
            overlay={overlay}
            draw={drawStrip}
            tooltip={(i) => <HoverCard s={s} c={c} i={i} />}
          />
        </main>

        <aside className="flex min-h-0 flex-col border-l border-line bg-panel">
          <div className="grid gap-5 border-b border-line p-5">
            {s.status === 'waiting' ? (
              <Waiting s={s} big={big} />
            ) : (
              <>
                {!big && (s.status === 'done' ? <RailSummary s={s} /> : <Readouts s={s} />)}
                <Phases s={s} />
              </>
            )}
          </div>
          {s.status !== 'waiting' && <Timeline s={s} />}
          {s.status === 'done' && (
            <div className="border-t border-line p-5 pb-20">
              <RoastedWeightField s={s} />
            </div>
          )}
        </aside>
      </div>

      {menu && <ClickMenu s={s} at={menu} onClose={() => setMenu(null)} />}
    </div>
  )
}

function Waiting({ s, big }: { s: Session; big: boolean }) {
  return (
    <>
      <div className="grid gap-4">
        <span className="text-sm font-semibold">Next roast</span>
        <NextRoastFields s={s} />
      </div>
      {!big && (
        <div className="grid grid-cols-2 gap-4">
          <Readout label="BT" value={fmt1(s.live.bt)} unit="°C" color={C.bt} size="lg" />
          <Readout label="ET" value={fmt1(s.live.et)} unit="°C" color={C.et} size="lg" />
        </div>
      )}
      <Button variant="primary" className="py-3 text-base" onClick={() => s.mark('charge')}>
        Charge <Kbd className="border-black/30 text-black/60">C</Kbd>
      </Button>
      <p className="text-xs text-white/40">Charge is detected automatically; press it only if detection misses.</p>
    </>
  )
}

function RailSummary({ s }: { s: Session }) {
  const drop = s.events.find((e) => e.kind === 'drop')
  const fc = s.events.find((e) => e.kind === 'first_crack')
  return (
    <div className="grid grid-cols-2 gap-x-4 gap-y-5">
      <div className="col-span-2">
        <Readout label="Total time" value={fmtClock(s.roastEnd)} size="xl" />
      </div>
      <Readout label="Development time ratio" value={s.devRatio !== undefined ? s.devRatio.toFixed(1) : '—'} unit="%" color={C.ror} size="lg" />
      <Readout label="Development" value={fc ? fmtClock(s.roastEnd - fc.t) : '—'} size="lg" />
      <Readout label="Drop BT" value={fmt1(drop ? liveAt(drop.t)?.[1] : undefined)} unit="°C" color={C.bt} />
      <Readout label="Weight loss" value={s.weightLoss !== undefined ? s.weightLoss.toFixed(1) : '—'} unit="%" />
    </div>
  )
}

// A's chart options plus dots on each curve under the cursor.
const withPoints = (build: () => ReturnType<typeof mainOptions> | ReturnType<typeof stripOptions>) => () => {
  const o = build()
  return { ...o, cursor: { ...o.cursor, points: { size: 9, width: 2, fill: '#0b0c0e' } } }
}
const mainWithPoints = withPoints(mainOptions)
const stripWithPoints = withPoints(stripOptions)

// What the hover card shows at one instant: live values, the reference's
// values and the difference, and any event or note right there.
function HoverCard({ s, c, i }: { s: Session; c: Columns; i: number }) {
  const x = c.x[i]
  const live = c.bt[i] !== null
  const ref = c.refBt[i] !== null
  if (!live && !ref) return null
  const t = s.status === 'waiting' ? x + s.now : x
  const isNow = live && t === s.now
  const here = s.status === 'waiting' ? [] : s.events.filter((e) => Math.abs(e.t - x) <= 2)
  const notes = s.status === 'waiting' ? [] : s.notes.filter((n) => Math.abs(n.t - x) <= 3)
  const row = (label: string, color: string, v: number | null, r: number | null, unit = '') =>
    !live ? (
      <div className="contents">
        <span className="text-white/50">{label}</span>
        <span className="col-span-3 text-right font-bold" style={{ color: fade(color, 0.7) }}>
          {r !== null ? `${fmt1(r)}${unit}` : '—'}
        </span>
      </div>
    ) : (
    <div className="contents">
      <span className="text-white/50">{label}</span>
      <span className="text-right font-bold" style={{ color }}>
        {v !== null ? `${fmt1(v)}${unit}` : '—'}
      </span>
      <span className="text-right text-white/40">{r !== null ? fmt1(r) : ''}</span>
      <span className="text-right text-white/60">{v !== null && r !== null ? fmtDelta(v - r) : ''}</span>
    </div>
    )
  return (
    <div className="grid w-60 gap-2 font-mono text-xs">
      <div className="flex items-baseline justify-between">
        <span className="tnum text-base font-bold">{s.status === 'waiting' ? (x === 0 ? 'now' : `${fmtClock(x)} ago`) : fmtClock(x)}</span>
        <span className="font-sans text-[11px] text-white/40">{isNow ? 'now' : live ? '' : s.ref ? `${s.ref.label} only` : ''}</span>
      </div>
      <div className="tnum grid grid-cols-[auto_1fr_1fr_1fr] gap-x-3 gap-y-1">
        {live && (
          <>
            <span />
            <span className="text-right font-sans text-[10px] text-white/35 uppercase">now</span>
            <span className="text-right font-sans text-[10px] text-white/35 uppercase">{ref ? 'ref' : ''}</span>
            <span className="text-right font-sans text-[10px] text-white/35 uppercase">{ref ? 'diff' : ''}</span>
          </>
        )}
        {row('BT', C.bt, c.bt[i], c.refBt[i], '°')}
        {row('ET', C.et, c.et[i], c.refEt[i], '°')}
        {row('RoR', C.ror, c.ror[i], c.refRor[i])}
      </div>
      {live && (
        <div className="flex gap-4 border-t border-white/10 pt-2">
          <span style={{ color: C.gas }}>gas {c.gas[i]?.toFixed(0)}%</span>
          <span style={{ color: C.air }}>air {c.fan[i]?.toFixed(0)}%</span>
        </div>
      )}
      {(here.length > 0 || notes.length > 0) && (
        <div className="grid gap-1 border-t border-white/10 pt-2 font-sans">
          {here.map((e) => (
            <span key={e.kind} className="text-accent">
              {EVENT_LABEL[e.kind]} · {fmtClock(e.t)}
            </span>
          ))}
          {notes.map((n) => (
            <span key={n.id} className="text-white/85">
              “{n.text}”
            </span>
          ))}
        </div>
      )}
      {live && s.status !== 'waiting' && <span className="font-sans text-[10px] text-white/35">Click to mark an event or add a note</span>}
    </div>
  )
}

type Entry =
  | { type: 'event'; t: number; kind: EventKind; source: string }
  | { type: 'note'; t: number; text: string; id: number; n: number }

// Variant C's timeline, with the "now" row carrying the next mark.
function Timeline({ s }: { s: Session }) {
  const end = useRef<HTMLDivElement>(null)
  const entries: Entry[] = [
    ...s.events.map((e) => ({ type: 'event' as const, t: e.t, kind: e.kind, source: e.source })),
    ...s.notes.map((n, i) => ({ type: 'note' as const, t: n.t, text: n.text, id: n.id, n: i + 1 })),
  ].sort((a, b) => a.t - b.t)
  const upcoming =
    s.status === 'done'
      ? []
      : s.refEvents.filter((e) => e.kind !== 'charge' && !s.events.some((x) => x.kind === e.kind) && e.t > s.now)
  const others = MARKABLE.filter((k) => k !== s.nextEvent && !s.events.some((e) => e.kind === k))

  useEffect(() => {
    end.current?.scrollIntoView({ block: 'nearest' })
  }, [entries.length])

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4 pb-20">
      <div className="flex items-baseline justify-between">
        <Label>Timeline</Label>
        <span className="text-[11px] text-white/30">click the chart to mark the past</span>
      </div>
      <ol className="relative mt-3 grid gap-0.5 border-l border-white/10 pl-4">
        {entries.map((e) => (
          <li key={e.type === 'note' ? `n${e.id}` : `e${e.kind}`} className="group relative flex items-baseline gap-3 py-1.5 text-sm">
            <span className={cn('absolute top-3 -left-[21px] size-2.5 rounded-full', e.type === 'note' ? 'bg-white' : 'bg-accent')} />
            <span className="tnum w-11 shrink-0 font-mono text-xs text-white/50">{fmtClock(e.t)}</span>
            {e.type === 'event' ? (
              <>
                <span className="font-semibold">{EVENT_LABEL[e.kind]}</span>
                <span className="tnum font-mono text-xs text-bt">{fmt1(liveAt(e.t)?.[1])}°</span>
                <span className="text-[10px] text-white/30 uppercase">{e.source}</span>
                {MARKABLE.includes(e.kind) && (
                  <button className="ml-auto text-white/0 group-hover:text-white/40 hover:!text-white" onClick={() => s.unmark(e.kind)}>
                    ✕
                  </button>
                )}
              </>
            ) : (
              <>
                <span className="flex-1 text-white/85">
                  <span className="mr-1.5 inline-grid size-4 place-items-center rounded-full bg-white text-[9px] font-bold text-black">{e.n}</span>
                  {e.text}
                </span>
                <button className="text-white/0 group-hover:text-white/40 hover:!text-white" onClick={() => s.removeNote(e.id)}>
                  ✕
                </button>
              </>
            )}
          </li>
        ))}
        {s.status === 'recording' && (
          <li className="relative grid gap-2 py-2">
            <span className="absolute top-3.5 -left-[22px] size-3 animate-pulse rounded-full border-2 border-accent bg-bg" />
            <div className="flex items-baseline gap-3 text-sm">
              <span className="tnum w-11 font-mono text-xs text-accent">{fmtClock(s.now)}</span>
              <span className="text-accent">now</span>
            </div>
            <div className="flex flex-wrap gap-1.5 pl-14">
              {s.nextEvent && s.nextEvent !== 'drop' && (
                <Button variant="primary" className="h-8 px-3 text-xs" onClick={() => s.mark(s.nextEvent!)}>
                  {EVENT_LABEL[s.nextEvent]}
                  {EVENT_HINT[s.nextEvent] && <span className="opacity-60">{EVENT_HINT[s.nextEvent]}</span>}
                  <Kbd className="border-black/30 text-black/60">{EVENT_KEY[s.nextEvent]}</Kbd>
                </Button>
              )}
              {others
                .filter((k) => k !== 'second_crack' || s.events.some((e) => e.kind === 'first_crack'))
                .map((k) => (
                  <Button key={k} variant="subtle" className="h-8 px-3 text-xs" onClick={() => s.mark(k)}>
                    {EVENT_LABEL[k]} <Kbd>{EVENT_KEY[k]}</Kbd>
                  </Button>
                ))}
              <Button variant="outline" className="h-8 px-3 text-xs" onClick={() => s.mark('drop')}>
                Drop <Kbd>X</Kbd>
              </Button>
            </div>
          </li>
        )}
        {upcoming.map((e) => (
          <li key={`u${e.kind}`} className="relative flex items-baseline gap-3 py-1.5 text-sm text-white/30">
            <span className="absolute top-3 -left-[21px] size-2.5 rounded-full border border-white/25 bg-bg" />
            <span className="tnum w-11 font-mono text-xs">{fmtClock(e.t)}</span>
            <span>{EVENT_LABEL[e.kind]}</span>
            <span className="text-[10px] uppercase">in reference</span>
          </li>
        ))}
      </ol>
      <div ref={end} />
    </div>
  )
}
