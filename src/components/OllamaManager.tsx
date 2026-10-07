import { useState, useSyncExternalStore } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Cpu, Download, ExternalLink, Star, Trash2, X } from "lucide-react";
import { api, errorMessage, newRequestId, type CatalogModel, type Fit, type LookupResult, type Provider } from "../ipc";
import { Badge, Button, ErrorNote, inputClass, Modal, Spinner } from "../ui";

// Downloads keep running when the dialog is closed, so their state lives outside React.
type Dl = { requestId: string; status: string; completed: number; total: number; error?: string };
let downloads: Record<string, Dl> = {};
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());
const setDl = (name: string, d: Dl | null) => {
  const next = { ...downloads };
  if (d) next[name] = d;
  else delete next[name];
  downloads = next;
  emit();
};
function useDownloads() {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => downloads,
  );
}

const fitBadge: Record<Fit, { tone: "good" | "warn" | "bad"; label: string }> = {
  fits: { tone: "good", label: "✓ fits" },
  tight: { tone: "warn", label: "⚠ tight" },
  "too-big": { tone: "bad", label: "✗ too big" },
};

const accelLabel = { "apple-silicon": "Apple Silicon GPU", nvidia: "NVIDIA GPU", cpu: "CPU only (no GPU)" };

function Quality({ n }: { n: number }) {
  return (
    <span className="inline-flex" title={`Quality ${n}/5`}>
      {Array.from({ length: 5 }).map((_, i) => (
        <span key={i} className={`mx-px inline-block size-1.5 rounded-full ${i < n ? "bg-accent" : "bg-line"}`} />
      ))}
    </span>
  );
}

function formatSecs(s: number) {
  return s < 90 ? `~${s}s` : `~${Math.round(s / 60)} min`;
}

export function OllamaManager({ provider, onClose }: { provider: Provider; onClose: () => void }) {
  const qc = useQueryClient();
  const dls = useDownloads();
  const catalog = useQuery({ queryKey: ["ollamaCatalog", provider.id], queryFn: () => api.ollamaCatalog(provider.id) });
  const [showAll, setShowAll] = useState(false);
  const [custom, setCustom] = useState("");
  const [lookup, setLookup] = useState<LookupResult | null>(null);
  const [lookupBusy, setLookupBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["ollamaCatalog", provider.id] });
    qc.invalidateQueries({ queryKey: ["providers"] });
    qc.invalidateQueries({ queryKey: ["models", provider.id] });
  };

  async function download(name: string) {
    const requestId = newRequestId();
    setDl(name, { requestId, status: "starting", completed: 0, total: 0 });
    try {
      await api.ollamaPull(requestId, provider.id, name, (p) =>
        setDl(name, {
          requestId,
          status: p.status,
          completed: p.completed ?? downloads[name]?.completed ?? 0,
          total: p.total ?? downloads[name]?.total ?? 0,
        }),
      );
      setDl(name, null);
      refresh();
    } catch (e) {
      const msg = errorMessage(e);
      if (msg === "Cancelled") setDl(name, null);
      else setDl(name, { requestId, status: "failed", completed: 0, total: 0, error: msg });
    }
  }

  async function remove(name: string) {
    setError(null);
    setConfirmDelete(null);
    try {
      await api.ollamaDelete(provider.id, name);
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function checkCustom() {
    setLookupBusy(true);
    setLookup(null);
    setError(null);
    try {
      setLookup(await api.ollamaLookup(custom));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLookupBusy(false);
    }
  }

  const sys = catalog.data?.system;
  const models = (catalog.data?.models ?? []).filter((m) => showAll || m.fit !== "too-big" || m.installed);
  const hiddenCount = (catalog.data?.models.length ?? 0) - models.length;

  function Row({ m }: { m: CatalogModel }) {
    const d = dls[m.name];
    const pct = d && d.total ? Math.round((d.completed / d.total) * 100) : 0;
    return (
      <li className={`py-3 ${m.fit === "too-big" ? "opacity-60" : ""}`}>
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-medium">{m.label}</span>
              <span className="font-mono text-[11px] text-muted">{m.name}</span>
              {m.recommended && (
                <Badge tone="accent">
                  <Star className="mr-0.5 size-3" /> recommended for this computer
                </Badge>
              )}
              {m.installed && <Badge tone="good">installed</Badge>}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted">
              <Badge tone={fitBadge[m.fit].tone}>{fitBadge[m.fit].label}</Badge>
              <span>
                {m.sizeGb.toFixed(1)} GB download · needs ~{m.memoryNeededGb.toFixed(0)} GB memory
              </span>
              <span className="flex items-center gap-1">
                quality <Quality n={m.quality} />
              </span>
              {m.fit !== "too-big" && (
                <span title="Rough estimate for this computer">
                  ~{m.estTokensPerSec.toFixed(0)} words/s · {formatSecs(m.estGradeSeconds)} to grade an answer
                </span>
              )}
              <span>good for: {m.goodFor.join(", ")}</span>
            </div>
            {m.note && <p className="mt-1 text-xs text-muted">{m.note}</p>}
            {d && !d.error && (
              <div className="mt-2">
                <div className="mb-1 flex justify-between text-[11px] text-muted">
                  <span>{d.status}</span>
                  {d.total > 0 && (
                    <span>
                      {(d.completed / 1e9).toFixed(2)} / {(d.total / 1e9).toFixed(2)} GB · {pct}%
                    </span>
                  )}
                </div>
                <div className="h-1.5 overflow-hidden rounded bg-sunken">
                  <div className="h-full bg-accent transition-all" style={{ width: `${pct}%` }} />
                </div>
              </div>
            )}
            {d?.error && <p className="mt-1 text-xs text-bad">{d.error}</p>}
          </div>
          <div className="flex shrink-0 items-center gap-1">
            {d && !d.error ? (
              <Button variant="ghost" onClick={() => api.cancel(d.requestId)} title="Cancel download (it resumes next time)">
                <X className="size-4" /> Cancel
              </Button>
            ) : m.installed ? (
              confirmDelete === m.name ? (
                <Button variant="danger" onClick={() => remove(m.name)}>
                  Delete {m.sizeGb.toFixed(1)} GB?
                </Button>
              ) : (
                <Button variant="ghost" onClick={() => setConfirmDelete(m.name)} aria-label={`Delete ${m.name}`}>
                  <Trash2 className="size-4" />
                </Button>
              )
            ) : (
              <Button variant={m.recommended ? "primary" : "secondary"} onClick={() => download(m.name)}>
                <Download className="size-4" /> Download
              </Button>
            )}
          </div>
        </div>
      </li>
    );
  }

  return (
    <Modal title="Download local models" onClose={onClose} wide>
      {sys && (
        <div className="mb-4 flex items-start gap-3 rounded-lg bg-sunken px-4 py-3 text-sm">
          <Cpu className="mt-0.5 size-4 shrink-0 text-muted" />
          <div>
            <div className="font-medium">
              This computer: {sys.totalRamGb.toFixed(0)} GB RAM · {sys.cpuCores} cores · {accelLabel[sys.accelerator]}
            </div>
            <div className="text-xs text-muted">
              Models up to ~{sys.usableForModelsGb.toFixed(0)} GB of memory can run here
              {sys.accelerator === "cpu" && " — on the CPU, so expect answers to take tens of seconds"}. Speeds are rough
              estimates; downloads continue if you close this window.
            </div>
          </div>
        </div>
      )}
      {catalog.isLoading && <Spinner />}
      <ErrorNote error={catalog.error ? errorMessage(catalog.error) : error} />

      <ul className="divide-y divide-line">
        {models.map((m) => (
          <Row key={m.name} m={m} />
        ))}
      </ul>
      {hiddenCount > 0 && (
        <button className="mt-1 text-xs text-accent hover:underline" onClick={() => setShowAll(!showAll)}>
          {showAll ? "Hide models that are too big" : `Show ${hiddenCount} models too big for this computer`}
        </button>
      )}

      <div className="mt-5 border-t border-line pt-4">
        <div className="mb-2 flex items-center gap-2 text-sm font-medium">
          Any other model
          <button
            className="inline-flex items-center gap-0.5 text-xs font-normal text-accent hover:underline"
            onClick={() => openUrl("https://ollama.com/library")}
          >
            browse ollama.com/library <ExternalLink className="size-3" />
          </button>
        </div>
        <div className="flex gap-2">
          <input
            className={`${inputClass} font-mono`}
            placeholder="e.g. qwen2.5-coder:7b"
            value={custom}
            onChange={(e) => {
              setCustom(e.target.value);
              setLookup(null);
            }}
            onKeyDown={(e) => e.key === "Enter" && custom.trim() && checkCustom()}
          />
          <Button busy={lookupBusy} disabled={!custom.trim()} onClick={checkCustom}>
            Check
          </Button>
        </div>
        {lookup && (
          <div className="mt-2 flex items-center gap-3 rounded-md bg-sunken px-3 py-2 text-sm">
            <span className="font-mono">{lookup.name}</span>
            <span className="text-xs text-muted">
              {lookup.sizeGb.toFixed(1)} GB · needs ~{lookup.memoryNeededGb.toFixed(0)} GB
            </span>
            <Badge tone={fitBadge[lookup.fit].tone}>{fitBadge[lookup.fit].label}</Badge>
            {dls[lookup.name] && !dls[lookup.name].error ? (
              <span className="ml-auto text-xs text-muted">
                downloading…{" "}
                {dls[lookup.name].total ? Math.round((dls[lookup.name].completed / dls[lookup.name].total) * 100) : 0}%
              </span>
            ) : (
              <Button className="ml-auto" onClick={() => download(lookup.name)}>
                <Download className="size-4" /> Download
              </Button>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}
