// PROTOTYPE, throwaway. Three structurally different layouts of the live
// roast screen on one page, switchable with ?variant= (A, B, C; D merges A and C after the first round). All share the
// same replayed simulator roast; only the layout differs.
import { useState } from 'react'
import { useRoastSession } from './lib/session'
import { PrototypeSwitcher } from './PrototypeSwitcher'
import { VariantA } from './variants/VariantA'
import { VariantB } from './variants/VariantB'
import { VariantC } from './variants/VariantC'
import { VariantD } from './variants/VariantD'

const VARIANTS = [
  { key: 'A', name: 'Cockpit', C: VariantA },
  { key: 'B', name: 'Big screen', C: VariantB },
  { key: 'C', name: 'Logbook', C: VariantC },
  { key: 'D', name: 'Cockpit + timeline', C: VariantD },
]

export function App() {
  const [variant, setVariant] = useState(() => new URLSearchParams(location.search).get('variant') ?? 'A')
  const s = useRoastSession()
  const change = (key: string) => {
    const url = new URL(location.href)
    url.searchParams.set('variant', key)
    history.replaceState(null, '', url)
    setVariant(key)
  }
  const V = (VARIANTS.find((v) => v.key === variant) ?? VARIANTS[0]).C
  return (
    <>
      <V key={variant} s={s} />
      <PrototypeSwitcher variants={VARIANTS} current={variant} onChange={change} s={s} />
    </>
  )
}
