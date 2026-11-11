// PROTOTYPE, throwaway. Thin React wrapper around uPlot: builds the plot once,
// pushes new data on every render, and redraws overlays (events, notes, phase
// bands) from a ref so the plot never has to be rebuilt. An optional hover
// card follows the cursor, rendered in a portal so only it re-renders.
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import uPlot from 'uplot'

export const C = {
  bt: '#f5a524',
  et: '#7aa2f7',
  ror: '#2dd4bf',
  gas: '#c084fc',
  air: '#a3e635',
  axis: 'rgba(255,255,255,0.45)',
  grid: 'rgba(255,255,255,0.06)',
  text: 'rgba(255,255,255,0.85)',
  muted: 'rgba(255,255,255,0.4)',
}

export const fade = (hex: string, a: number) => {
  const n = parseInt(hex.slice(1), 16)
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`
}

export interface ChartClick {
  t: number // roast clock under the pointer
  value: number | null // value of the first y scale under the pointer
  clientX: number
  clientY: number
}

type Options = Omit<uPlot.Options, 'width' | 'height'>

export function Chart<O>({
  options,
  data,
  overlay,
  draw,
  onClick,
  tooltip,
  className,
}: {
  options: () => Options
  data: uPlot.AlignedData
  overlay: O
  draw?: (u: uPlot, overlay: O) => void
  onClick?: (c: ChartClick) => void
  tooltip?: (idx: number) => ReactNode // content of the hover card for data index idx
  className?: string
}) {
  const box = useRef<HTMLDivElement>(null)
  const plot = useRef<uPlot | null>(null)
  const latest = useRef({ overlay, draw, onClick, tooltip })
  latest.current = { overlay, draw, onClick, tooltip }
  const [hover, setHover] = useState<{ idx: number; x: number; y: number } | null>(null)

  useEffect(() => {
    const el = box.current!
    const o = options()
    const yScale = Object.keys(o.scales ?? {}).find((k) => k !== 'x') ?? 'y'
    let pointerIn = false
    const u = new uPlot(
      {
        ...o,
        ...contentSize(el),
        hooks: {
          ...o.hooks,
          draw: [(u) => latest.current.draw?.(u, latest.current.overlay)],
          setCursor: [
            (u) => {
              // Synced charts move their cursor too; only the one under the mouse shows a card.
              const { idx, left, top } = u.cursor
              if (!latest.current.tooltip || !pointerIn || idx == null || left == null || left < 0) return setHover(null)
              const r = u.over.getBoundingClientRect()
              setHover({ idx, x: r.left + left, y: r.top + (top ?? 0) })
            },
          ],
        },
      },
      data,
      el,
    )
    u.over.style.cursor = 'crosshair'
    u.over.addEventListener('mouseenter', () => (pointerIn = true))
    u.over.addEventListener('mouseleave', () => {
      pointerIn = false
      setHover(null)
    })
    u.over.addEventListener('click', (e) => {
      latest.current.onClick?.({
        t: Math.round(u.posToVal(e.offsetX, 'x')),
        value: u.posToVal(e.offsetY, yScale),
        clientX: e.clientX,
        clientY: e.clientY,
      })
    })
    plot.current = u
    const ro = new ResizeObserver(() => u.setSize(contentSize(el)))
    ro.observe(el)
    return () => {
      ro.disconnect()
      u.destroy()
      plot.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    // Scales have fixed ranges, so a full setData only redraws.
    plot.current?.setData(data)
  }, [data])

  const card = hover && tooltip ? tooltip(hover.idx) : null
  return (
    <div ref={box} className={className}>
      {card &&
        createPortal(
          <div
            className="pointer-events-none fixed z-30 rounded-xl border border-white/10 bg-panel-2/95 p-3 shadow-2xl shadow-black/60 backdrop-blur"
            style={{
              left: hover!.x > innerWidth - 300 ? hover!.x - 276 : hover!.x + 16,
              top: Math.min(hover!.y + 16, innerHeight - 220),
            }}
          >
            {card}
          </div>,
          document.body,
        )}
    </div>
  )
}

function contentSize(el: HTMLElement) {
  const cs = getComputedStyle(el)
  return {
    width: Math.max(0, el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight)),
    height: Math.max(0, el.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom)),
  }
}

// --- Common axis and series builders ----------------------------------------

const font = (px: number) => `${px}px "Space Mono", ui-monospace, monospace`

export function clockAxis(show = true): uPlot.Axis {
  return {
    scale: 'x',
    show,
    stroke: C.axis,
    font: font(11),
    grid: { stroke: C.grid, width: 1 },
    ticks: { stroke: C.grid, width: 1, size: 4 },
    incrs: [30, 60, 120, 300],
    values: (_u, splits) =>
      splits.map((s) => {
        const a = Math.abs(s)
        return `${s < 0 ? '−' : ''}${Math.floor(a / 60)}:${String(a % 60).padStart(2, '0')}`
      }),
  }
}

export function valueAxis(scale: string, opts: Partial<uPlot.Axis> = {}): uPlot.Axis {
  return {
    scale,
    stroke: C.axis,
    font: font(11),
    size: 44,
    grid: { stroke: C.grid, width: 1 },
    ticks: { show: false },
    ...opts,
  }
}

export function line(label: string, scale: string, stroke: string, opts: Partial<uPlot.Series> = {}): uPlot.Series {
  return { label, scale, stroke, width: 2, points: { show: false }, spanGaps: false, ...opts }
}

export const stepped = uPlot.paths.stepped!({ align: 1 })

export const baseOptions: Pick<Options, 'legend' | 'cursor'> = {
  legend: { show: false },
  cursor: {
    y: false,
    points: { show: false },
    drag: { x: false, y: false, setScale: false },
  },
}
