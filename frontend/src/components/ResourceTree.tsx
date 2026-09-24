import { cn, ui } from "../ui";
import { Link } from "react-router-dom";
import { useTargets } from "../queries";

export function ResourceTree() {
  const targets = useTargets();
  const items = targets.data ?? [];
  return <section className="grid gap-px border-y border-slate-600 py-2 text-xs max-[720px]:hidden" aria-label="Ressourcenbaum">
    <div className="flex items-center justify-between px-2 pb-1 text-[10px] font-bold uppercase tracking-wider text-slate-400"><span>Zugänge &amp; Agenten</span><span className={ui.muted}>{items.length}</span></div>
    <Link className="flex items-center gap-2 px-2 py-1 text-slate-200 hover:bg-slate-700" to="/targets">Zugangsprofile</Link>
    {items.map((target) => <Link className="flex items-center gap-2 px-2 py-1 text-slate-200 hover:bg-slate-700" key={target.id} to="/targets"><span className={cn(ui.statusDot, target.state === "managed" ? ui.statusDotOk : ui.statusDotWarn)} />{target.name}<span className="ml-auto text-[var(--muted)]">{target.kind}</span></Link>)}
    {!items.length && !targets.isLoading ? <span className="px-2 py-1 text-[var(--muted)]">Keine Zugänge angelegt</span> : null}
  </section>;
}
