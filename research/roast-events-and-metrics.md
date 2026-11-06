# Roast events and metrics: how roasting software defines and detects them

Research for issue #4. Question: how do established roasting tools and the
published literature define charge, turning point, dry end, first crack,
second crack and drop, and the derived metrics (development time, DTR, phase
percentages, RoR, weight loss)? Which events are automatic and which are
marked by hand, and what detection rules are published?

Sources are vendor documentation (Cropster help center, Artisan docs and
blog), Scott Rao's book and blog, and one peer-reviewed paper. Artisan's
source code was **not** read (GPLv3). Where a vendor does not publish a rule,
this note says so rather than guessing.

## TL;DR

- **Only charge and drop are commonly detected automatically**, and both from
  the bean temperature (BT) curve: charge = a sharp BT drop, drop = a second,
  smaller BT drop. Cropster publishes example thresholds (7 °C for start, 3 °C
  over 5 s for end). Artisan publishes no numbers and calls its heuristic
  fragile.
- **Turning point (TP) is computed, not marked**: the BT minimum after charge.
- **Dry end / colour change, first crack and second crack are sensory events**
  (colour, sound). Software marks them by hand, or "auto-marks" them when BT
  crosses a temperature the user sets. No tool claims to *detect* them from the
  signal; Cropster *predicts* first crack with ML but does not detect it.
- **Development time** = first crack start → drop. **DTR** = development
  time / total roast time (charge → drop). Rao's guideline: 20–25 %.
- **Phases**: drying (charge → dry end), Maillard (dry end → first crack),
  development/finishing (first crack → drop). Percentages are each phase's
  share of total roast time.
- **RoR** = d(BT)/dt in °C/min, computed over a window. Smoothing choices
  differ a lot between tools, and that changes how a curve looks.

## Events

### Charge

- **Definition**: the moment green beans enter the hot drum; start of the
  roast clock. Loring: "The temperature at which the roast begins" [L1].
- **Automatic or manual**: both, in every tool. Manual is a button press;
  automatic is opt-in.
- **Cropster** (temperature-based auto-start, a Roasting Intelligence feature):
  "detecting the sharp temperature drops that occur when beans are loaded".
  Example threshold: a drop of **7 °C / 13 °F**, with a **30 s** window. BT must
  be stable (not already falling) at charge, or the start may be missed.
  Electrical noise that produces a spike meeting the threshold can start the
  roast falsely [C1]. Cropster also has a machine-event-based variant that uses
  the roaster's own signals instead of temperature (beta, needs a compatible
  machine connector) [C1].
- **Artisan** ("autoCHARGE", Config » Events): sets CHARGE automatically. The
  docs only state a precondition: recording must run for 5× the sampling
  interval before charging so there is a baseline [A1]. An alternative is the
  "Charge Timer", which sets CHARGE a fixed number of seconds after START [A1].
  The heuristic itself is not documented; the release notes say the signal
  "is often rather smooth which makes detection challenging" and "can also be
  easily confused with measuring noise", and v2.10.2 added a "Sensitive" mode
  that catches more charges at the cost of false triggers on noisy BT [A2]. A
  forum description by a developer says the charge is placed at the point where
  the drop *began*, i.e. back-dated, and that a BT floor applies (users mention
  95 °C vs 150 °C) [F1] (forum, lower trust).
- **Carmomaq context**: charge is mechanical on the Caloratto; the PLC does not
  report it (`docs/context.md`), so a button plus BT-based detection is the
  only option, as in both tools above.

### Turning point (TP)

- **Definition**: "the moment the BT temperature starts to rise after the
  initial drop" (Artisan blog) [A3]; "the time and temperature at which the Bean
  Temperature first increases after the Charge point" (Loring) [L1]. In other
  words, the BT minimum after charge.
- **Automatic or manual**: automatic. It falls out of the data; no source
  describes marking it by hand. Artisan shows TP as an annotation (and on
  background profiles since v1.0) [A3]. Artisan's quick-start mentions a
  setting to mark TP [A0]. No vendor publishes the minimum-finding rule (how
  long BT must keep rising before the minimum is confirmed).
- **Why it matters**: the BT probe reads low after charge because it is
  surrounded by cold beans, so TP is a probe artefact as much as a bean
  temperature. Loring treats TP as an expectation that depends on charge
  temperature, burner, batch size and preheat, not a target [L1].

### Dry end / yellowing / colour change

- **Definition**: the point where beans turn yellow; end of the drying phase
  and start of the Maillard phase. Artisan: drying runs "to the point where the
  beans turn yellow (DRY)" [A4]. Loring calls it "Color Change ... (also know as
  yellowing)" [L1]. Cropster calls it "Color change" [C2].
- **Automatic or manual**: manual by default (the operator looks at the trier).
  Both tools can auto-mark it by temperature:
  - Artisan "AutoDRY" generates DRY "once bean temperature reaches the Drying
    limit" the user sets in Config » Phases [A4].
  - Cropster "Auto-mark development events" marks colour change, first crack
    and second crack, "triggered based on bean temperature" [C2]. The help page
    does not say which temperatures; presumably the user's or the reference
    profile's values.
- **No published default threshold** in vendor docs. ~150 °C BT is the figure
  most commonly quoted by roasters (forums, blogs), but it is a convention and
  is probe- and machine-specific.
- **Disagreement**: Rao has a section titled "The Myth of the Drying Phase" in
  *The Coffee Roaster's Companion* [R1], arguing beans keep losing moisture well
  past yellowing, so "drying" is a misnomer for the first phase. Several
  sources say the phase names are "somewhat misleading" or still evolving.

### First crack (start)

- **Definition**: the audible event when steam pressure inside the bean cracks
  it. Münchow et al.: "the accumulated steam pressure causes the beans to crack
  and expel steam along with other volatile compounds"; time to first crack is
  "when a popping sound is first heard" [P1]. Rao is stricter: first crack
  begins "the moment the operator hears more than one or two isolated pops"
  [R1]. Cropster: "expanding beans release a burst of evaporating moisture"
  [C3].
- **Automatic or manual**: manual (by ear) in practice. Optional
  temperature auto-mark in both Artisan and Cropster, as for dry end [C2][A4].
  Cropster's AI "First crack prediction" shows a prediction window about three
  minutes ahead; it is a forecast, not a detection [C3]. No tool documents
  acoustic detection.
- **Disagreement**: first pop (Münchow) vs. "more than one or two pops" (Rao).
  This moves FC by seconds and therefore moves development time and DTR.
- First crack end (FCe) exists as an Artisan event but is rarely used for
  metrics.

### Second crack (start)

- **Definition**: a second, quieter, faster crackling at darker roast levels.
  Loring lists it as an optional anchor [L1]; Cropster can auto-mark it by
  temperature [C2]; Artisan has SC START / SC END buttons.
- **Automatic or manual**: manual, or temperature auto-mark. Not used in any
  standard metric; Rao uses it only as an upper bound for drop on lighter
  roasts [R2].

### Drop

- **Definition**: beans leave the drum (door opened); end of the roast clock.
  Loring calls the planned endpoint "End of Roast" [L1].
- **Automatic or manual**: both, like charge.
- **Cropster**: end of roast = a *second* temperature drop; example threshold:
  BT "consistently decreases by **3 °C / 5 °F over 5 seconds**" [C1]. The
  same noise caveat applies.
- **Artisan** ("autoDROP"): heuristic undocumented; it shares the "Standard /
  Sensitive" modes and the same caveats as autoCHARGE [A2]. Users report noisy
  BT fooling it; the fix offered is to turn it off [F2].
- **Note**: once beans leave, the BT probe is in the empty drum, so its reading
  falls fast. This is the signal both tools key on. A heat cut or airflow change
  late in the roast can produce a smaller dip, which is why a short persistence
  window (Cropster's 5 s) is used.

### Other events in the software

Artisan also has FC END, SC END, COOL END and custom events (gas, air, drum
sliders recorded as events) [A1]. Cropster records comments and machine
setting changes on the curve [C2].

## Derived metrics

### Development time

- **Definition**: time from first crack start to drop. Münchow et al.:
  "development time ... corresponding to the time elapsed from the first crack
  to the end of the roasting process" [P1]; equivalently total time minus time
  to first crack.
- **Requires**: FC start (usually manual) and drop.

### DTR (development time ratio)

- **Definition**: development time as a percentage of total roast time
  (charge → drop). Rao, who coined it: "the roast time from the onset of first
  crack to the end of a roast should make up 20%–25% of total roast time. Put
  another way, first crack should begin at between 75%–80% of total roast
  time" [R1].
- **Targets**: Rao still prefers 20–25 % for roasts between end of first crack
  and start of second crack, but notes it is "simply one of many indicators",
  can be lower (≈15 %) with a lot of burner per kg (e.g. 10 kg in a Probat
  UG22), and that "roast degree and development, not DTR, are what determine
  body" [R2]. Artisan's docs mention "Many people are focused on a 20%-25%
  development ratio" [A4]. Cropster lists DTR as a roast field without
  publishing a formula [C4].
- **Disagreement**: none on the formula; disagreement is on the target and on
  how much DTR matters versus roast colour. Münchow et al. found colour the
  stronger flavour predictor, but at equal colour development time mattered
  more than time to first crack [P1].

### Phase percentages (drying / Maillard / development)

- **Boundaries** (Artisan) [A4]:
  - Drying: charge → DRY (yellow).
  - Maillard: DRY → FCs.
  - Finishing (development): FCs → DROP.
- Each percentage = phase duration / total roast time. Artisan's Phases LCDs
  show time, percentage or temperature per phase; percentage after FCs is the
  live DTR [A4]. Cropster shows drying, Maillard and development phases under
  the curve [C4].
- **Phase limits**: Artisan can take them from typed temperatures, from the
  events actually marked ("Auto Adjusted"), or from a background profile [A4].
- **Disagreement**: phase names differ (Finishing vs Development vs
  "post-crack"), and some roasters start drying at TP rather than charge.
  Artisan and Rao measure from charge.

### RoR (rate of rise)

- **Definition**: the rate of change of BT, normally °C/min (or °F/min).
  Cropster: "the current increase of the bean temperature over some time called
  the time expression" [C5]. Royal Coffee: "change rate, or ... derivative" [W1].
- **Window and smoothing**: this is where tools differ the most.
  - Artisan: delta = Δtemp / Δtime over the "Delta Span" (max 30 s; spans under
    2× the sampling interval have no effect). Extra options: Smooth Curves
    (pre-delta, max 5), RoR Smoothing (averaging, max 40, adds time shift),
    Polyfit (least-squares line fit, smoother without delay), Optimal Smoothing
    post-roast (centred, no time shift, only for a finished roast), Drop Spikes,
    Smooth Spikes, RoR limits. RoR is only drawn after CHARGE [A5]. Default
    sampling is 2 s [A6].
  - Cropster (RI5): three presets, "Recommended", "Sensitive", "Noise
    smoothing". The "time expression" (default 60 s) only changes the unit
    (°C/60 s, °C/30 s), not the curve shape [C5]. In 2019 the option was a
    10–60 s *interval* that did change smoothing [R3]; the current behaviour
    is different, so older advice does not carry over.
  - Rao: no "Smooth Curves/Smooth Deltas" in Artisan; let the delta span be the
    only smoothing; "use the lowest practical delta span or ROR interval"; a
    longer window for trends (he used 30 s in Cropster), a shorter one for
    events like a crash (10 s) [R3]. Use the fastest probe setting available [R4].
- **Shape rules**: Rao's "declining RoR" principle: bean RoR should fall
  steadily; a "flick" (RoR acceleration late in first crack, typically around
  16–17 % DTR in a 10–14 min drum roast) and a "crash" early in first crack are
  faults [R4]. Royal Coffee disputes that this works for all coffees and roast
  styles [W1].
- **Roastro note**: `internal/roast/ror.go` already computes RoR as a
  least-squares slope over a sliding window, the same idea as Artisan's Polyfit
  option and causal (no time shift beyond half the window).

### Weight loss

- **Definition**: (green weight − roasted weight) / green weight × 100. Cropster
  tracks start and end weight per batch and keeps an average weight loss per
  profile; it can also pre-fill the end weight from an expected loss [C6].
  Typical range 15–22 % per Münchow et al. [P1]. Not derived from the curve;
  needs two weighings.

## Summary table

| Event | Definition | Automatic or manual | Published detection rule |
|---|---|---|---|
| Charge | Beans enter drum; roast clock starts | Manual button, or opt-in auto | Sharp BT drop. Cropster example: 7 °C drop, 30 s window, BT must be stable before. Artisan: undocumented heuristic, needs ≥5 samples of baseline, Standard/Sensitive modes |
| Turning point | BT minimum after charge | Automatic (computed) | BT stops falling and starts rising; no vendor publishes a confirmation window |
| Dry end / colour change | Beans turn yellow; drying → Maillard | Manual (visual), or auto-mark at a set BT | User-set BT threshold (Artisan AutoDRY, Cropster auto-mark). ~150 °C is a convention, not a standard |
| First crack start | First audible cracks | Manual (by ear), or auto-mark at a set BT | User-set BT threshold only. Cropster predicts it with ML; nobody detects it from the signal |
| Second crack start | Second, faster crackling | Manual, or auto-mark at a set BT | User-set BT threshold only |
| Drop | Beans leave drum; roast clock ends | Manual button, or opt-in auto | Second BT drop. Cropster example: 3 °C over 5 s. Artisan: undocumented, Standard/Sensitive |

| Metric | Formula |
|---|---|
| Development time | drop − FC start |
| DTR | development time / (drop − charge) |
| Phase % | phase duration / (drop − charge); drying = charge→DE, Maillard = DE→FCs, development = FCs→drop |
| RoR | dBT/dt in °C/min over a window; smoothing varies by tool |
| Weight loss | (green − roasted) / green |

## Implications for Roastro

1. Charge and drop: offer a button and an opt-in BT detector with
   configurable threshold and persistence (start from Cropster's published
   7 °C / 3 °C-over-5 s), back-date charge to the onset of the drop, and always
   let the user override. The simulator's `/charge` and `/drop` give ground
   truth to test against.
2. TP: compute it, never ask for it; confirm the minimum only after BT has
   risen for a few samples, and recompute after the roast.
3. DE, FC, SC: manual buttons first; optional "auto-mark at BT ≥ X" per
   profile. Do not promise detection.
4. DTR and phase percentages are pure functions of event times; they belong in
   `internal/roast` next to RoR.
5. Store raw samples and compute RoR on read, so smoothing settings can change
   after a roast (Artisan does this; a shared file looks different under
   different settings [A5]).

## Sources

- [A0] Artisan, Quick-Start Guide. <https://docs.artisan.tools/desktop/getting-started/quick-start-guide> (formerly artisan-scope.org/docs/quick-start-guide)
- [A1] Artisan, Configuration » Events. <https://docs.artisan.tools/desktop/configuration/events>
- [A2] Artisan blog, "Artisan v2.10.2" (Feb 2024), CHARGE/DROP detection heuristic and Sensitive mode. <https://artisan-roasterscope.blogspot.com/2024/02/artisan-v2102.html>
- [A3] Artisan blog, "Roasting with Phidgets" (Dec 2017), TP definition. <https://artisan-roasterscope.blogspot.com/2017/12/roasting-with-phidgets.html>; Artisan v1.0 notes (TP annotation on background profiles) <https://artisan-roasterscope.blogspot.com/2017/02/artisan-v10.html>
- [A4] Artisan, Configuration » Phases. <https://docs.artisan.tools/desktop/configuration/phases>
- [A5] Artisan, Configuration » Curves (RoR, filters). <https://docs.artisan.tools/desktop/configuration/curves>
- [A6] Artisan, Configuration » Sampling. <https://docs.artisan.tools/desktop/configuration/sampling>
- [C1] Cropster help, "How does the Temperature based auto-start and end detection work". <https://help.cropster.com/en/knowledge/how-does-the-temperature-based-auto-start-and-end-detection-work>; machine-event variant: <https://help.cropster.com/en/knowledge/how-does-machine-event-based-auto-start-and-end-detection-work>
- [C2] Cropster help, "How to set comments in Roasting Intelligence (RI)" (auto-mark development events). <https://help.cropster.com/en/knowledge/how-to-set-comments-in-roasting-intelligence-ri>
- [C3] Cropster help, "How to use first crack prediction". <https://help.cropster.com/en/knowledge/how-to-use-first-crack-prediction>
- [C4] Cropster, Roast features. <https://cropster.com/products/roast/features>
- [C5] Cropster help, "About Rate of Rise (RoR)". <https://help.cropster.com/en/knowledge/about-rate-of-rise-ror>
- [C6] Cropster help, "About weight loss" / "How to track weight loss". <https://help.cropster.com/en/knowledge/about-weight-loss>
- [L1] Loring, Roast Profile help. <https://loring.com/helpfiles/RA/RoastProfile.html>
- [R1] Scott Rao, *The Coffee Roaster's Companion* (2014), sample pages ("Three Commandments", DTR, first-crack footnote; contents incl. "The Myth of the Drying Phase"). <https://cdn.shopify.com/s/files/1/0958/8450/files/CRC_Sample_Pages.pdf>
- [R2] Scott Rao, "Development Time Ratio" (2016). <https://www.scottrao.com/blog/2016/8/25/development-time-ratio>
- [R3] Scott Rao, "Idle noise, ROR intervals, and analyzing roast curves" (2019). <https://www.scottrao.com/blog/2019/7/3/how-to-manage-roast-software-settings>
- [R4] Scott Rao, "The Flick, updated for 2021" (Cropster blog). <https://www.cropster.com/blog-post/the-flick-updated-for-2021/>
- [P1] Münchow, Alstrup, Steen, Giacalone, "Roasting Conditions and Coffee Flavor: A Multi-Study Empirical Investigation", *Beverages* 2020, 6, 29. doi:10.3390/beverages6020029
- [W1] Royal Coffee, "Rate of Rise: Where Do You Stand?". <https://royalcoffee.com/rate-of-rise-where-do-you-stand/>
- [F1] Home-Barista forum, "Artisan AUTO Charge not consistent" (lower trust). <https://www.home-barista.com/home-roasting/artisan-auto-charge-not-consistent-t61149-10.html>
- [F2] Artisan GitHub Discussions, auto DROP fooled by noisy BT (lower trust; discussion text only, not code). <https://github.com/artisan-roaster-scope/artisan/discussions/1960>

### Gaps

- Artisan's autoCHARGE / autoDROP / TP rules are not documented outside the
  source code, which we must not read. Treat them as unknown.
- Cropster does not publish the BT thresholds used by "Auto-mark development
  events", nor its DTR formula (assumed the standard one).
- No SCA standard defines these events; SCA-affiliated work (Münchow et al.)
  uses the same industry definitions.
