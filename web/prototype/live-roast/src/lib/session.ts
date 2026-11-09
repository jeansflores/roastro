// PROTOTYPE, throwaway. Replays a recorded simulator roast and keeps the
// roast's events and notes in memory, the way the real backend would push
// them. Shared by every variant so they differ only in layout.
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  COFFEES,
  LIVE,
  LIVE_END,
  LIVE_START,
  X_MAX,
  X_MIN,
  dtr,
  liveAt,
  phasesOf,
  refAt,
  refById,
  refDrop,
  refEvents,
  refNotes,
  turningPoint,
  type EventKind,
  type Note,
  type RoastEvent,
} from './roast'

export type Status = 'waiting' | 'recording' | 'done'

export interface NextRoast {
  coffee: string
  greenKg: string
  refId: string | null
  roastedBy: string
}

export function useRoastSession() {
  const [now, setNow] = useState(-90)
  const [playing, setPlaying] = useState(true)
  const [speed, setSpeed] = useState(5)
  const [manual, setManual] = useState<RoastEvent[]>([])
  const [notes, setNotes] = useState<Note[]>([])
  const [next, setNext] = useState<NextRoast>({ coffee: COFFEES[0], greenKg: '4.600', refId: '41', roastedBy: 'Lucas' })
  const [roastedKg, setRoastedKg] = useState('')

  // Events as the backend would report them: automatic Charge/Drop unless the
  // operator pressed the button first, computed Turning point, manual marks.
  const events = useMemo(() => {
    const ev: RoastEvent[] = []
    if (now < 0) return ev
    ev.push(manual.find((e) => e.kind === 'charge') ?? { kind: 'charge', t: 0, source: 'auto' })
    const tp = turningPoint(now, liveAt)
    if (tp) ev.push(tp)
    for (const e of manual) if (e.kind !== 'charge' && e.kind !== 'drop' && e.t <= now) ev.push(e)
    const md = manual.find((e) => e.kind === 'drop')
    if (md) ev.push(md)
    else if (now >= LIVE.drop_t) ev.push({ kind: 'drop', t: LIVE.drop_t, source: 'auto' })
    return ev.sort((a, b) => a.t - b.t)
  }, [now, manual])

  const drop = events.find((e) => e.kind === 'drop')
  const status: Status = now < 0 ? 'waiting' : drop ? 'done' : 'recording'
  const end = drop ? Math.min(drop.t + 30, LIVE_END) : LIVE_END

  // Replay clock: one recorded second per tick.
  useEffect(() => {
    if (!playing) return
    const id = setInterval(() => setNow((n) => Math.min(n + 1, end)), 1000 / speed)
    return () => clearInterval(id)
  }, [playing, speed, end])
  useEffect(() => {
    if (now >= end) setPlaying(false)
  }, [now, end])

  const jump = useCallback((t: number) => {
    const target = Math.max(LIVE_START + 30, Math.min(t, LIVE_END))
    setNow(target)
    // Pretend the operator marked what a jump skips over.
    setManual((m) => {
      const kept = m.filter((e) => e.t <= target && target >= 0)
      for (const kind of ['dry_end', 'first_crack'] as const) {
        const truth = LIVE.truth[kind]
        if (target > truth + 2 && !kept.some((e) => e.kind === kind)) kept.push({ kind, t: truth, source: 'manual' })
      }
      return kept
    })
    setNotes((ns) => ns.filter((n) => n.t <= target))
    setPlaying(true)
  }, [])

  const mark = useCallback(
    (kind: EventKind, at?: number) => {
      if (kind === 'charge') {
        if (now < 0) {
          setNow(0)
          setManual([{ kind: 'charge', t: 0, source: 'manual' }])
        }
        return
      }
      if (now < 0) return
      const t = Math.max(1, Math.min(Math.round(at ?? now), now))
      setManual((m) => [...m.filter((e) => e.kind !== kind), { kind, t, source: 'manual' }])
    },
    [now],
  )

  const unmark = useCallback((kind: EventKind) => {
    setManual((m) => m.filter((e) => e.kind !== kind))
  }, [])

  const addNote = useCallback(
    (text: string, at?: number) => {
      if (!text.trim()) return
      const t = Math.round(Math.min(at ?? now, now))
      setNotes((ns) => [...ns, { id: Date.now(), t, text: text.trim() }].sort((a, b) => a.t - b.t))
    },
    [now],
  )
  const removeNote = useCallback((id: number) => setNotes((ns) => ns.filter((n) => n.id !== id)), [])

  const ref = refById(next.refId)
  const tp = events.find((e) => e.kind === 'turning_point')
  const row = liveAt(now)
  const refRow = ref && status !== 'waiting' ? refAt(ref, now) : undefined
  const refTp = ref ? refEvents(ref).find((e) => e.kind === 'turning_point') : undefined
  const showRor = status !== 'waiting' && tp !== undefined && now > tp.t
  const showRefRor = refRow !== undefined && refTp !== undefined && now > refTp.t

  const live = {
    t: now,
    bt: row?.[1],
    et: row?.[2],
    ror: showRor ? row?.[3] : undefined,
    gas: row?.[4],
    fan: row?.[5],
    drum: row?.[6],
    refBt: refRow?.[1],
    refEt: refRow?.[2],
    refRor: showRefRor ? refRow?.[3] : undefined,
  }

  const roastEnd = drop?.t ?? Math.max(now, 0)
  const phases = phasesOf(events, roastEnd)
  const devRatio = dtr(events, roastEnd)

  const nextEvent: EventKind | undefined =
    status === 'waiting'
      ? 'charge'
      : status === 'done'
        ? undefined
        : !events.some((e) => e.kind === 'dry_end')
          ? 'dry_end'
          : !events.some((e) => e.kind === 'first_crack')
            ? 'first_crack'
            : 'drop'

  const green = parseFloat(next.greenKg)
  const roasted = parseFloat(roastedKg)
  const weightLoss = green > 0 && roasted > 0 ? ((green - roasted) / green) * 100 : undefined

  return {
    now,
    status,
    playing,
    setPlaying,
    speed,
    setSpeed,
    jump,
    end,
    events,
    notes,
    mark,
    unmark,
    addNote,
    removeNote,
    next,
    setNext,
    roastedKg,
    setRoastedKg,
    weightLoss,
    ref,
    refEvents: ref ? refEvents(ref) : [],
    refNotes: ref ? refNotes(ref) : [],
    refDrop: ref ? refDrop(ref) : undefined,
    live,
    phases,
    devRatio,
    nextEvent,
    roastEnd,
  }
}

export type Session = ReturnType<typeof useRoastSession>

// Chart columns on the fixed X_MIN..X_MAX grid. While waiting for Charge the
// live trace is drawn relative to "now", which sits at zero.
export interface Columns {
  x: number[]
  bt: (number | null)[]
  et: (number | null)[]
  ror: (number | null)[]
  gas: (number | null)[]
  fan: (number | null)[]
  refBt: (number | null)[]
  refEt: (number | null)[]
  refRor: (number | null)[]
}

export function columns(s: Session): Columns {
  const c: Columns = { x: [], bt: [], et: [], ror: [], gas: [], fan: [], refBt: [], refEt: [], refRor: [] }
  const offset = s.status === 'waiting' ? s.now : 0
  const tp = s.events.find((e) => e.kind === 'turning_point')
  const refTp = s.refEvents.find((e) => e.kind === 'turning_point')
  for (let x = X_MIN; x <= X_MAX; x++) {
    c.x.push(x)
    const t = x + offset
    const row = t <= s.now ? liveAt(t) : undefined
    c.bt.push(row?.[1] ?? null)
    c.et.push(row?.[2] ?? null)
    c.ror.push(row && tp && t > tp.t && s.status !== 'waiting' ? row[3] : null)
    c.gas.push(row?.[4] ?? null)
    c.fan.push(row?.[5] ?? null)
    const rr = s.ref && s.refDrop !== undefined && x <= s.refDrop + 30 ? refAt(s.ref, x) : undefined
    c.refBt.push(rr?.[1] ?? null)
    c.refEt.push(rr?.[2] ?? null)
    c.refRor.push(rr && refTp && x > refTp.t ? rr[3] : null)
  }
  return c
}
