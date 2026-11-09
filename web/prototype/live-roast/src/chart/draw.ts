// PROTOTYPE, throwaway. Canvas overlays drawn in uPlot's draw hook. All
// coordinates are canvas pixels (CSS px × devicePixelRatio).
import type uPlot from 'uplot'
import { C, fade } from './Chart'
import type { Phase } from '../lib/roast'

const dpr = () => window.devicePixelRatio || 1
const font = (px: number, weight = 400, mono = true) =>
  `${weight} ${px * dpr()}px ${mono ? '"Space Mono", ui-monospace' : '"Space Grotesk", ui-sans-serif'}`

const xPos = (u: uPlot, t: number) => u.valToPos(t, 'x', true)
const inX = (u: uPlot, px: number) => px >= u.bbox.left - 1 && px <= u.bbox.left + u.bbox.width + 1

function pill(ctx: CanvasRenderingContext2D, x: number, y: number, text: string, fg: string, bg: string, align: 'left' | 'center' | 'right' = 'center') {
  const pad = 6 * dpr()
  const w = ctx.measureText(text).width + pad * 2
  const h = 18 * dpr()
  const left = align === 'center' ? x - w / 2 : align === 'left' ? x : x - w
  ctx.fillStyle = bg
  ctx.beginPath()
  ctx.roundRect(left, y, w, h, h / 2)
  ctx.fill()
  ctx.fillStyle = fg
  ctx.textBaseline = 'middle'
  ctx.textAlign = 'left'
  ctx.fillText(text, left + pad, y + h / 2 + 0.5)
  return { left, w, h }
}

export const PHASE_TINT: Record<Phase['key'], string> = {
  drying: 'rgba(122,162,247,0.05)',
  maillard: 'rgba(245,165,36,0.06)',
  development: 'rgba(45,212,191,0.07)',
}

export function phaseBands(u: uPlot, phases: Phase[], labels = true) {
  const { ctx } = u
  const { top, height } = u.bbox
  ctx.save()
  for (const p of phases) {
    const a = Math.max(xPos(u, p.from), u.bbox.left)
    const b = Math.min(xPos(u, p.to), u.bbox.left + u.bbox.width)
    if (b <= a) continue
    ctx.fillStyle = PHASE_TINT[p.key]
    ctx.fillRect(a, top, b - a, height)
    if (labels && b - a > 70 * dpr()) {
      ctx.font = font(10, 500, false)
      ctx.fillStyle = C.muted
      ctx.textAlign = 'left'
      ctx.textBaseline = 'bottom'
      ctx.fillText(p.label.toUpperCase(), a + 6 * dpr(), top + height - 6 * dpr())
    }
  }
  ctx.restore()
}

export interface Mark {
  t: number
  label: string
  color?: string
  strong?: boolean
}

// Vertical event lines with a label pill at the top. Rows alternate so close
// events do not overlap.
export function eventLines(u: uPlot, marks: Mark[], opts: { pills?: boolean; dash?: number[] } = {}) {
  const { ctx } = u
  const { top, height } = u.bbox
  ctx.save()
  ctx.font = font(10.5, 700)
  marks.forEach((m, i) => {
    const x = xPos(u, m.t)
    if (!inX(u, x)) return
    ctx.strokeStyle = m.color ?? 'rgba(255,255,255,0.35)'
    ctx.lineWidth = (m.strong ? 1.5 : 1) * dpr()
    ctx.setLineDash((opts.dash ?? [4, 4]).map((d) => d * dpr()))
    ctx.beginPath()
    ctx.moveTo(x, top)
    ctx.lineTo(x, top + height)
    ctx.stroke()
    ctx.setLineDash([])
    if (opts.pills !== false) {
      const row = i % 2
      pill(ctx, x, top + 4 * dpr() + row * 22 * dpr(), m.label, '#0b0c0e', m.color ?? 'rgba(255,255,255,0.85)')
    }
  })
  ctx.restore()
}

// Small ticks along the bottom edge for the reference roast's events.
export function refTicks(u: uPlot, marks: Mark[]) {
  const { ctx } = u
  const bottom = u.bbox.top + u.bbox.height
  ctx.save()
  ctx.font = font(9.5, 400)
  ctx.textAlign = 'center'
  ctx.textBaseline = 'bottom'
  for (const m of marks) {
    const x = xPos(u, m.t)
    if (!inX(u, x)) continue
    ctx.strokeStyle = 'rgba(255,255,255,0.25)'
    ctx.lineWidth = dpr()
    ctx.setLineDash([2 * dpr(), 3 * dpr()])
    ctx.beginPath()
    ctx.moveTo(x, bottom)
    ctx.lineTo(x, bottom - 26 * dpr())
    ctx.stroke()
    ctx.setLineDash([])
    ctx.fillStyle = 'rgba(255,255,255,0.35)'
    ctx.fillText(m.label, x, bottom - 28 * dpr())
  }
  ctx.restore()
}

export interface Dot {
  t: number
  v: number
  label: string
  ghost?: boolean
}

// Numbered note markers sitting on a curve.
export function noteDots(u: uPlot, dots: Dot[], scale: string) {
  const { ctx } = u
  ctx.save()
  ctx.font = font(10, 700)
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  for (const d of dots) {
    const x = xPos(u, d.t)
    if (!inX(u, x)) continue
    const y = u.valToPos(d.v, scale, true) - 16 * dpr()
    const r = 8 * dpr()
    ctx.fillStyle = d.ghost ? 'rgba(255,255,255,0.12)' : '#f4f4f5'
    ctx.strokeStyle = d.ghost ? 'rgba(255,255,255,0.3)' : '#0b0c0e'
    ctx.lineWidth = 1.5 * dpr()
    ctx.beginPath()
    ctx.arc(x, y, r, 0, Math.PI * 2)
    ctx.fill()
    ctx.stroke()
    ctx.beginPath()
    ctx.moveTo(x, y + r)
    ctx.lineTo(x, y + 14 * dpr())
    ctx.stroke()
    ctx.fillStyle = d.ghost ? 'rgba(255,255,255,0.6)' : '#0b0c0e'
    ctx.fillText(d.label, x, y + 0.5)
  }
  ctx.restore()
}

export function nowLine(u: uPlot, t: number, label?: string) {
  const { ctx } = u
  const x = xPos(u, t)
  if (!inX(u, x)) return
  ctx.save()
  ctx.strokeStyle = fade(C.bt, 0.6)
  ctx.lineWidth = dpr()
  ctx.beginPath()
  ctx.moveTo(x, u.bbox.top)
  ctx.lineTo(x, u.bbox.top + u.bbox.height)
  ctx.stroke()
  if (label) {
    ctx.font = font(10.5, 700)
    pill(ctx, x + 6 * dpr(), u.bbox.top + 4 * dpr(), label, '#0b0c0e', C.bt, 'left')
  }
  ctx.restore()
}

export interface Tag {
  t: number
  v: number
  scale: string
  color: string
  text: string
}

// Value tags attached to the head of each live line, pushed apart vertically.
export function headTags(u: uPlot, tags: Tag[]) {
  const { ctx } = u
  ctx.save()
  ctx.font = font(14, 700)
  const h = 24 * dpr()
  const placed = tags
    .map((g) => ({ g, x: xPos(u, g.t), y: u.valToPos(g.v, g.scale, true) - h / 2 }))
    .filter((p) => inX(u, p.x))
    .sort((a, b) => a.y - b.y)
  for (let i = 1; i < placed.length; i++) {
    if (placed[i].y < placed[i - 1].y + h + 2) placed[i].y = placed[i - 1].y + h + 2
  }
  for (const p of placed) {
    const pad = 8 * dpr()
    const w = ctx.measureText(p.g.text).width + pad * 2
    const left = p.x + 10 * dpr()
    ctx.fillStyle = p.g.color
    ctx.beginPath()
    ctx.roundRect(left, p.y, w, h, 6 * dpr())
    ctx.fill()
    ctx.beginPath()
    ctx.moveTo(p.x + 2 * dpr(), u.valToPos(p.g.v, p.g.scale, true))
    ctx.lineTo(left, p.y + h / 2)
    ctx.strokeStyle = p.g.color
    ctx.lineWidth = 1.5 * dpr()
    ctx.stroke()
    ctx.fillStyle = '#0b0c0e'
    ctx.textAlign = 'left'
    ctx.textBaseline = 'middle'
    ctx.fillText(p.g.text, left + pad, p.y + h / 2 + 0.5)
  }
  ctx.restore()
}

export function pin(u: uPlot, t: number, label: string) {
  const { ctx } = u
  const x = xPos(u, t)
  if (!inX(u, x)) return
  ctx.save()
  ctx.strokeStyle = '#f4f4f5'
  ctx.lineWidth = 1.5 * dpr()
  ctx.beginPath()
  ctx.moveTo(x, u.bbox.top)
  ctx.lineTo(x, u.bbox.top + u.bbox.height)
  ctx.stroke()
  ctx.font = font(10.5, 700)
  pill(ctx, x, u.bbox.top + u.bbox.height - 24 * dpr(), label, '#0b0c0e', '#f4f4f5')
  ctx.restore()
}
