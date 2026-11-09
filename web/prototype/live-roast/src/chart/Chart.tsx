// PROTOTYPE, throwaway. Thin React wrapper around uPlot: builds the plot once,
// pushes new data on every render, and redraws overlays (events, notes, phase
// bands) from a ref so the plot never has to be rebuilt.
import { useEffect, useRef } from 'react'
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
  className,
}: {
  options: () => Options
  data: uPlot.AlignedData
  overlay: O
  draw?: (u: uPlot, overlay: O) => void
  onClick?: (c: ChartClick) => void
  className?: string
}) {
  const box = useRef<HTMLDivElement>(null)
  const plot = useRef<uPlot | null>(null)
  const latest = useRef({ overlay, draw, onClick })
  latest.current = { overlay, draw, onClick }

  useEffect(() => {
    const el = box.current!
    const o = options()
    const yScale = Object.keys(o.scales ?? {}).find((k) => k !== 'x') ?? 'y'
    const u = new uPlot(
      {
        ...o,
        ...contentSize(el),
        hooks: {
          ...o.hooks,
          draw: [(u) => latest.current.draw?.(u, latest.current.overlay)],
        },
      },
      data,
      el,
    )
    u.over.style.cursor = 'crosshair'
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
  })

  return <div ref={box} className={className} />
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
