// PROTOTYPE, throwaway. Recorded simulator data and roast helpers shared by
// every variant. Nothing here is a design decision; the variants are.
import data from '../data/roasts.json'

// Sample columns: roast clock (s), bt, et, ror (°C/min), gas_fb (%), fan (%), drum (%).
export type Row = number[]
interface Recorded {
  samples: Row[]
  drop_t: number
  truth: Record<'turning_point' | 'dry_end' | 'first_crack', number>
}

export const LIVE = data.live as Recorded
const REF = data.reference as Recorded

export const LIVE_START = LIVE.samples[0][0]
export const LIVE_END = LIVE.samples[LIVE.samples.length - 1][0]

// Fixed chart window: 1 min before Charge to 13 min.
export const X_MIN = -60
export const X_MAX = 780

export function liveAt(t: number): Row | undefined {
  return LIVE.samples[Math.round(t) - LIVE_START]
}

export type EventKind = 'charge' | 'turning_point' | 'dry_end' | 'first_crack' | 'second_crack' | 'drop'
export const EVENT_ORDER: EventKind[] = ['charge', 'turning_point', 'dry_end', 'first_crack', 'second_crack', 'drop']
export const EVENT_LABEL: Record<EventKind, string> = {
  charge: 'Charge',
  turning_point: 'Turning point',
  dry_end: 'Dry end',
  first_crack: 'First crack',
  second_crack: 'Second crack',
  drop: 'Drop',
}
export const EVENT_SHORT: Record<EventKind, string> = {
  charge: 'CH',
  turning_point: 'TP',
  dry_end: 'DE',
  first_crack: 'FC',
  second_crack: 'SC',
  drop: 'DROP',
}
export const EVENT_HINT: Partial<Record<EventKind, string>> = {
  dry_end: 'color change',
  first_crack: 'first pops',
}
export const EVENT_KEY: Partial<Record<EventKind, string>> = {
  charge: 'C',
  dry_end: 'D',
  first_crack: 'F',
  second_crack: 'S',
  drop: 'X',
}
export const MARKABLE: EventKind[] = ['dry_end', 'first_crack', 'second_crack']

export interface RoastEvent {
  kind: EventKind
  t: number
  source: 'auto' | 'manual' | 'computed'
}
export interface Note {
  id: number
  t: number
  text: string
}

// --- Reference roasts -------------------------------------------------------

export interface Reference {
  id: string
  label: string
  date: string
  stretch: number // time factor applied to the one recorded reference curve
}
export const REFERENCES: Reference[] = [
  { id: '41', label: 'Roast #41', date: '2 Nov', stretch: 1 },
  { id: '37', label: 'Roast #37', date: '28 Oct', stretch: 1.06 },
]
export const COFFEES = [
  'Bourbon Amarelo – Fazenda Santa Inês',
  'Catuaí Vermelho – Sítio Boa Vista',
  'Mundo Novo – Fazenda Recreio',
]

export function refById(id: string | null): Reference | undefined {
  return REFERENCES.find((r) => r.id === id)
}

export function refAt(ref: Reference, t: number): Row | undefined {
  const src = t > 0 ? t / ref.stretch : t
  const row = REF.samples[Math.round(src) - REF.samples[0][0]]
  if (!row) return undefined
  return [t, row[1], row[2], row[3] / ref.stretch, row[4], row[5], row[6]]
}

export function refEvents(ref: Reference): RoastEvent[] {
  const s = ref.stretch
  return [
    { kind: 'charge', t: 0, source: 'auto' },
    { kind: 'turning_point', t: Math.round(REF.truth.turning_point * s), source: 'computed' },
    { kind: 'dry_end', t: Math.round(REF.truth.dry_end * s), source: 'manual' },
    { kind: 'first_crack', t: Math.round(REF.truth.first_crack * s), source: 'manual' },
    { kind: 'drop', t: Math.round(REF.drop_t * s), source: 'auto' },
  ]
}

export function refNotes(ref: Reference): Note[] {
  const s = ref.stretch
  return [
    { id: -1, t: Math.round(330 * s), text: 'Gas 80 → 70, air 55' },
    { id: -2, t: Math.round(515 * s), text: 'Gas 70 → 50, air 65' },
    { id: -3, t: Math.round(585 * s), text: 'Loud first pops' },
  ]
}

export function refDrop(ref: Reference): number {
  return Math.round(REF.drop_t * ref.stretch)
}

// --- Computations -------------------------------------------------------------

// Turning point: lowest BT after Charge, confirmed once BT has risen 2 °C above it.
export function turningPoint(now: number, at: (t: number) => Row | undefined): RoastEvent | undefined {
  let min = Infinity
  let tMin = 0
  for (let t = 1; t <= Math.min(now, 300); t++) {
    const bt = at(t)?.[1]
    if (bt !== undefined && bt < min) {
      min = bt
      tMin = t
    }
  }
  const cur = at(now)?.[1]
  if (cur === undefined || cur < min + 2 || now - tMin < 5) return undefined
  return { kind: 'turning_point', t: tMin, source: 'computed' }
}

export interface Phase {
  key: 'drying' | 'maillard' | 'development'
  label: string
  from: number
  to: number
  open: boolean // still running
}

export function phasesOf(events: RoastEvent[], end: number): Phase[] {
  const t = (k: EventKind) => events.find((e) => e.kind === k)?.t
  const charge = t('charge')
  if (charge === undefined) return []
  const de = t('dry_end')
  const fc = t('first_crack')
  const drop = t('drop')
  const stop = drop ?? end
  const out: Phase[] = []
  out.push({ key: 'drying', label: 'Drying', from: charge, to: de ?? stop, open: de === undefined && drop === undefined })
  if (de !== undefined)
    out.push({ key: 'maillard', label: 'Maillard', from: de, to: fc ?? stop, open: fc === undefined && drop === undefined })
  if (fc !== undefined)
    out.push({ key: 'development', label: 'Development', from: fc, to: stop, open: drop === undefined })
  return out
}

export function dtr(events: RoastEvent[], end: number): number | undefined {
  const fc = events.find((e) => e.kind === 'first_crack')?.t
  if (fc === undefined) return undefined
  const stop = events.find((e) => e.kind === 'drop')?.t ?? end
  return stop > 0 ? ((stop - fc) / stop) * 100 : undefined
}

// --- Formatting ---------------------------------------------------------------

export function fmtClock(s: number): string {
  const sign = s < 0 ? '−' : ''
  const a = Math.abs(Math.round(s))
  return `${sign}${Math.floor(a / 60)}:${String(a % 60).padStart(2, '0')}`
}

export function fmt1(v: number | undefined | null): string {
  return v === undefined || v === null || Number.isNaN(v) ? '—' : v.toFixed(1)
}

export function fmtDelta(v: number | undefined): string {
  if (v === undefined || Number.isNaN(v)) return ''
  const s = v >= 0 ? '+' : '−'
  return `${s}${Math.abs(v).toFixed(1)}`
}
