import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CheckCircle2, Download, ExternalLink, FolderOpen, KeyRound, Plus, Trash2, XCircle, Zap } from "lucide-react";
import { OllamaManager } from "../components/OllamaManager";
import {
  api,
  errorMessage,
  type ImportPreview,
  type ModelInfo,
  type Preset,
  type Provider,
  type ProviderInput,
  type Role,
  type RoleLink,
  type TestResult,
} from "../ipc";
import { useAppInfo } from "../App";
import { Badge, Button, ErrorNote, Field, inputClass, Modal, Spinner } from "../ui";

function Section({ title, description, children }: { title: string; description?: string; children: React.ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="text-base font-semibold">{title}</h2>
      {description && <p className="mb-3 text-sm text-muted">{description}</p>}
      {children}
    </section>
  );
}

// ---------- providers ----------

function ProviderForm({ presets, initial, onClose }: { presets: Preset[]; initial?: Provider; onClose: () => void }) {
  const qc = useQueryClient();
  const [preset, setPreset] = useState<Preset | null>(
    initial ? (presets.find((p) => p.type === initial.providerType) ?? null) : null,
  );
  const [form, setForm] = useState<ProviderInput>(() =>
    initial
      ? {
          id: initial.id,
          providerType: initial.providerType,
          label: initial.label,
          baseUrl: initial.baseUrl,
          enabledModels: initial.enabledModels,
          options: initial.options,
        }
      : { providerType: "", label: "", baseUrl: "", enabledModels: [], options: {} },
  );
  const [key, setKey] = useState("");
  const saveM = useMutation({
    mutationFn: () => api.saveProvider({ ...form, apiKey: key.trim() ? key.trim() : undefined }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["providers"] });
      onClose();
    },
  });

  if (!preset) {
    return (
      <Modal title="Add an AI provider" onClose={onClose} wide>
        <div className="grid grid-cols-2 gap-2">
          {presets.map((p) => (
            <button
              key={p.type}
              className="rounded-lg border border-line p-3 text-left hover:border-accent"
              onClick={() => {
                setPreset(p);
                setForm({ ...form, providerType: p.type, label: p.label, baseUrl: p.baseUrl });
              }}
            >
              <div className="flex items-center gap-2 text-sm font-medium">
                {p.label}
                {p.local && <Badge tone="accent">local</Badge>}
              </div>
              {p.note && <div className="mt-1 text-xs text-muted">{p.note}</div>}
            </button>
          ))}
        </div>
      </Modal>
    );
  }

  const isOllama = preset.adapter === "ollama";
  return (
    <Modal
      title={initial ? `Edit ${initial.label}` : `Add ${preset.label}`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={saveM.isPending} onClick={() => saveM.mutate()}>
            Save
          </Button>
        </>
      }
    >
      <Field label="Name">
        <input className={inputClass} value={form.label} onChange={(e) => setForm({ ...form, label: e.target.value })} />
      </Field>
      <Field label="Base URL">
        <input
          className={inputClass}
          value={form.baseUrl}
          placeholder="https://…/v1"
          onChange={(e) => setForm({ ...form, baseUrl: e.target.value })}
        />
      </Field>
      {(preset.needsKey || preset.type === "custom") && (
        <Field
          label={initial?.hasKey ? "API key (saved — leave empty to keep it)" : "API key"}
          hint={
            <>
              Stored in your system keychain, never in the app's database or export files.{" "}
              {preset.docsUrl && (
                <button className="inline-flex items-center gap-0.5 text-accent hover:underline" onClick={() => openUrl(preset.docsUrl)}>
                  Get a key <ExternalLink className="size-3" />
                </button>
              )}
            </>
          }
        >
          <input className={inputClass} type="password" value={key} onChange={(e) => setKey(e.target.value)} />
        </Field>
      )}
      {isOllama && (
        <div className="grid grid-cols-2 gap-3">
          <Field label="Context window (tokens)" hint="Bigger = more of the chapter per question, more RAM.">
            <input
              className={inputClass}
              type="number"
              placeholder="8192"
              value={form.options.numCtx ?? ""}
              onChange={(e) => setForm({ ...form, options: { ...form.options, numCtx: e.target.value ? Number(e.target.value) : null } })}
            />
          </Field>
          <Field label="CPU threads" hint="Empty = half your cores, so the computer stays usable.">
            <input
              className={inputClass}
              type="number"
              placeholder="auto"
              value={form.options.numThread ?? ""}
              onChange={(e) => setForm({ ...form, options: { ...form.options, numThread: e.target.value ? Number(e.target.value) : null } })}
            />
          </Field>
        </div>
      )}
      <ErrorNote error={saveM.error ? errorMessage(saveM.error) : null} />
    </Modal>
  );
}

function formatPrice(m: ModelInfo) {
  if (m.promptPrice == null) return null;
  const f = (n: number) => (n === 0 ? "free" : `$${n < 1 ? n.toFixed(2) : n.toFixed(1)}`);
  return `${f(m.promptPrice)} in / ${f(m.completionPrice ?? 0)} out per 1M`;
}

function ModelPicker({ provider, onClose, onDownload }: { provider: Provider; onClose: () => void; onDownload: () => void }) {
  const qc = useQueryClient();
  const models = useQuery({ queryKey: ["models", provider.id], queryFn: () => api.providerModels(provider.id), staleTime: 300_000 });
  const [enabled, setEnabled] = useState(new Set(provider.enabledModels));
  const [filter, setFilter] = useState("");
  const [tests, setTests] = useState<Record<string, TestResult | string | "running">>({});

  const saveM = useMutation({
    mutationFn: () =>
      api.saveProvider({
        id: provider.id,
        providerType: provider.providerType,
        label: provider.label,
        baseUrl: provider.baseUrl,
        options: provider.options,
        enabledModels: [...enabled],
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["providers"] });
      onClose();
    },
  });

  async function test(id: string) {
    setTests((t) => ({ ...t, [id]: "running" }));
    try {
      const r = await api.testModel(provider.id, id);
      setTests((t) => ({ ...t, [id]: r }));
    } catch (e) {
      setTests((t) => ({ ...t, [id]: errorMessage(e) }));
    }
  }

  const list = (models.data ?? []).filter((m) => m.id.toLowerCase().includes(filter.toLowerCase()));

  return (
    <Modal
      title={`${provider.label} — models`}
      onClose={onClose}
      wide
      footer={
        <>
          <span className="mr-auto self-center text-xs text-muted">{enabled.size} selected</span>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={saveM.isPending} onClick={() => saveM.mutate()}>
            Save selection
          </Button>
        </>
      }
    >
      <p className="mb-3 text-sm text-muted">
        This list comes live from {provider.label}. Tick the models you want to use, then pick which does what under
        “What each model does”. <b>Try</b> sends one tiny request to check speed and whether the model returns clean JSON.
      </p>
      <input className={`${inputClass} mb-3`} placeholder="Filter models" value={filter} onChange={(e) => setFilter(e.target.value)} />
      {models.isLoading && <Spinner />}
      <ErrorNote error={models.error ? errorMessage(models.error) : null} />
      {models.data?.length === 0 && (
        <p className="text-sm text-muted">
          No models found.
        </p>
      )}
      {provider.adapter === "ollama" && (
        <div className="mb-3 flex items-center gap-3 rounded-md bg-accent-soft px-3 py-2 text-sm">
          <span className="flex-1">These are the models already on this computer. Want more?</span>
          <Button variant="primary" onClick={onDownload}>
            <Download className="size-4" /> Download models
          </Button>
        </div>
      )}
      <ul className="divide-y divide-line">
        {list.map((m) => {
          const t = tests[m.id];
          return (
            <li key={m.id} className="flex items-center gap-3 py-2">
              <input
                type="checkbox"
                checked={enabled.has(m.id)}
                onChange={(e) => {
                  const next = new Set(enabled);
                  if (e.target.checked) next.add(m.id);
                  else next.delete(m.id);
                  setEnabled(next);
                }}
              />
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-[13px]">{m.id}</div>
                <div className="flex flex-wrap gap-x-3 text-[11px] text-muted">
                  {m.parameterSize && <span>{m.parameterSize}</span>}
                  {m.quantization && <span>{m.quantization}</span>}
                  {m.sizeBytes != null && <span>{(m.sizeBytes / 1e9).toFixed(1)} GB</span>}
                  {m.contextLength != null && <span>{(m.contextLength / 1000).toFixed(0)}k context</span>}
                  {formatPrice(m) && <span>{formatPrice(m)}</span>}
                </div>
                {t && t !== "running" && (
                  <div className="mt-0.5 text-[11px]">
                    {typeof t === "string" ? (
                      <span className="text-bad">{t}</span>
                    ) : (
                      <span className={t.jsonOk ? "text-good" : "text-warn"}>
                        {(t.latencyMs / 1000).toFixed(1)}s · {t.jsonOk ? "clean JSON ✓" : "JSON output unreliable"}
                      </span>
                    )}
                  </div>
                )}
              </div>
              <Button variant="ghost" className="text-xs" busy={t === "running"} onClick={() => test(m.id)}>
                <Zap className="size-3.5" /> Try
              </Button>
            </li>
          );
        })}
      </ul>
    </Modal>
  );
}

function ProvidersSection() {
  const qc = useQueryClient();
  const presets = useQuery({ queryKey: ["presets"], queryFn: api.providerPresets, staleTime: Infinity });
  const providers = useQuery({ queryKey: ["providers"], queryFn: api.providers });
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<Provider | null>(null);
  const [picking, setPicking] = useState<Provider | null>(null);
  const [managing, setManaging] = useState<Provider | null>(null);
  const del = useMutation({
    mutationFn: api.deleteProvider,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["providers"] }),
  });

  return (
    <Section
      title="AI providers"
      description="Use a local model (free, private, slower on CPU) or any cloud provider with your own API key."
    >
      <div className="flex flex-col gap-2">
        {providers.data?.map((p) => (
          <div key={p.id} className="flex items-center gap-3 rounded-lg border border-line bg-panel px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-sm font-medium">
                {p.label}
                {p.needsKey &&
                  (p.hasKey ? (
                    <Badge tone="good">
                      <KeyRound className="mr-1 size-3" /> key saved
                    </Badge>
                  ) : (
                    <Badge tone="warn">needs a key on this PC</Badge>
                  ))}
              </div>
              <div className="truncate text-xs text-muted">
                {p.baseUrl} · {p.enabledModels.length} model{p.enabledModels.length === 1 ? "" : "s"} selected
              </div>
            </div>
            {p.adapter === "ollama" && (
              <Button onClick={() => setManaging(p)}>
                <Download className="size-4" /> Download models
              </Button>
            )}
            <Button variant="primary" onClick={() => setPicking(p)} disabled={p.needsKey && !p.hasKey}>
              Choose models
            </Button>
            <Button onClick={() => setEditing(p)}>Edit</Button>
            <Button variant="ghost" onClick={() => del.mutate(p.id)} aria-label="Remove provider">
              <Trash2 className="size-4" />
            </Button>
          </div>
        ))}
      </div>
      <Button className="mt-3" onClick={() => setAdding(true)} disabled={!presets.data}>
        <Plus className="size-4" /> Add provider
      </Button>
      {adding && presets.data && <ProviderForm presets={presets.data} onClose={() => setAdding(false)} />}
      {editing && presets.data && <ProviderForm presets={presets.data} initial={editing} onClose={() => setEditing(null)} />}
      {picking && (
        <ModelPicker
          provider={picking}
          onClose={() => setPicking(null)}
          onDownload={() => {
            setManaging(picking);
            setPicking(null);
          }}
        />
      )}
      {managing && <OllamaManager provider={managing} onClose={() => setManaging(null)} />}
    </Section>
  );
}

// ---------- roles ----------

function RoleRow({ role, options }: { role: Role; options: { value: string; label: string }[] }) {
  const qc = useQueryClient();
  const [chain, setChain] = useState<RoleLink[]>(role.chain);
  const setM = useMutation({
    mutationFn: (c: RoleLink[]) => api.setRole(role.role, c),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["roles"] }),
  });
  const encode = (l: RoleLink) => `${l.providerId}::${l.model}`;
  const update = (i: number, value: string) => {
    const next = [...chain];
    if (!value) next.splice(i, 1);
    else {
      const [providerId, ...rest] = value.split("::");
      next[i] = { providerId, model: rest.join("::") };
    }
    setChain(next);
    setM.mutate(next);
  };
  const slots = Math.min(chain.length + 1, 3);
  const missing = chain.filter((l) => !options.some((o) => o.value === encode(l)));

  return (
    <div className="rounded-lg border border-line bg-panel px-4 py-3">
      <div className="mb-2 text-sm font-medium">{role.label}</div>
      <div className="grid grid-cols-3 gap-2">
        {Array.from({ length: slots }).map((_, i) => (
          <select key={i} className={inputClass} value={chain[i] ? encode(chain[i]) : ""} onChange={(e) => update(i, e.target.value)}>
            <option value="">{i === 0 ? "Choose a model…" : "(optional) fallback…"}</option>
            {options.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
            {chain[i] && !options.some((o) => o.value === encode(chain[i])) && (
              <option value={encode(chain[i])}>{chain[i].model} (unavailable)</option>
            )}
          </select>
        ))}
      </div>
      {missing.length > 0 && (
        <p className="mt-2 text-xs text-warn">
          Not available on this computer: {missing.map((m) => m.model).join(", ")} — add its key or model here, or pick another.
        </p>
      )}
      <ErrorNote error={setM.error ? errorMessage(setM.error) : null} />
    </div>
  );
}

function RolesSection() {
  const roles = useQuery({ queryKey: ["roles"], queryFn: api.roles });
  const providers = useQuery({ queryKey: ["providers"], queryFn: api.providers });
  const options = useMemo(
    () =>
      (providers.data ?? [])
        .filter((p) => !p.needsKey || p.hasKey)
        .flatMap((p) => p.enabledModels.map((m) => ({ value: `${p.id}::${m}`, label: `${m} — ${p.label}` }))),
    [providers.data],
  );
  return (
    <Section
      title="What each model does"
      description="Pick a model per job. Fallbacks are tried in order if the first one fails (offline, rate-limited, bad output)."
    >
      {options.length === 0 && <p className="mb-2 text-sm text-warn">Add a provider and choose some models first.</p>}
      <div className="flex flex-col gap-2">
        {roles.data?.map((r) => (
          <RoleRow key={`${r.role}:${r.chain.length}`} role={r} options={options} />
        ))}
      </div>
    </Section>
  );
}

// ---------- data ----------

function DataSection() {
  const qc = useQueryClient();
  const info = useAppInfo();
  const [msg, setMsg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ path: string; data: ImportPreview } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  async function run(label: string, f: () => Promise<void>) {
    setBusy(label);
    setError(null);
    setMsg(null);
    try {
      await f();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const exportNow = () =>
    run("export", async () => {
      const date = new Date().toISOString().slice(0, 10);
      const path = await save({
        defaultPath: `bookshelf-progress-${date}.json`,
        filters: [{ name: "Bookshelf export", extensions: ["json"] }],
      });
      if (!path) return;
      await api.exportProgress(path);
      setMsg(`Exported to ${path}`);
    });

  const importStart = () =>
    run("import", async () => {
      const path = await open({ filters: [{ name: "Bookshelf export", extensions: ["json"] }] });
      if (typeof path !== "string") return;
      setPreview({ path, data: await api.importPreview(path) });
    });

  const importApply = (mode: "merge" | "replace") =>
    run(mode, async () => {
      if (!preview) return;
      const changes = await api.importApply(preview.path, mode);
      setPreview(null);
      await qc.invalidateQueries();
      const n = changes.reduce((s, c) => s + c.added + c.updated, 0);
      setMsg(`Import finished: ${n} records added or updated. A backup of your previous data was saved first.`);
    });

  const changeLibrary = () =>
    run("library", async () => {
      const dir = await open({ directory: true, title: "Choose your books folder" });
      if (typeof dir !== "string") return;
      await api.setLibraryRoot(dir);
      await qc.invalidateQueries();
      setMsg("Library folder changed and rescanned.");
    });

  return (
    <Section
      title="Your data"
      description="Progress, quiz history and settings live in a database on this computer. Use export/import to move them between computers (API keys are never exported)."
    >
      <div className="mb-3 rounded-lg border border-line bg-panel px-4 py-3 text-sm">
        <div className="flex items-center gap-2">
          <FolderOpen className="size-4 text-muted" />
          <span className="text-muted">Books folder:</span>
          <span className="truncate font-mono text-xs">{info.data?.libraryRoot}</span>
          <Button className="ml-auto" busy={busy === "library"} onClick={changeLibrary}>
            Change
          </Button>
        </div>
        <div className="mt-2 text-xs text-muted">
          App data: <span className="font-mono">{info.data?.dataDir}</span>
        </div>
      </div>
      <div className="flex gap-2">
        <Button variant="primary" busy={busy === "export"} onClick={exportNow}>
          Export progress…
        </Button>
        <Button busy={busy === "import"} onClick={importStart}>
          Import progress…
        </Button>
      </div>
      <div className="mt-3 flex flex-col gap-2">
        {msg && <div className="rounded-md bg-good/10 px-3 py-2 text-sm text-good">{msg}</div>}
        <ErrorNote error={error} />
      </div>
      {preview && (
        <Modal
          title="Import progress"
          onClose={() => setPreview(null)}
          footer={
            <>
              <Button variant="ghost" className="mr-auto text-bad" busy={busy === "replace"} onClick={() => importApply("replace")}>
                Replace everything
              </Button>
              <Button onClick={() => setPreview(null)}>Cancel</Button>
              <Button variant="primary" busy={busy === "merge"} onClick={() => importApply("merge")}>
                Merge
              </Button>
            </>
          }
        >
          <p className="mb-3 text-sm">
            Exported {new Date(preview.data.exportedAt).toLocaleString()}{" "}
            {preview.data.sameDevice ? "from this computer" : "from another computer"} (app v{preview.data.appVersion}).
          </p>
          <table className="mb-3 w-full text-sm">
            <thead>
              <tr className="text-left text-xs text-muted">
                <th className="py-1 font-medium">Data</th>
                <th className="py-1 font-medium">New</th>
                <th className="py-1 font-medium">Newer than yours</th>
                <th className="py-1 font-medium">Unchanged / older</th>
              </tr>
            </thead>
            <tbody>
              {preview.data.changes.map((c) => (
                <tr key={c.table} className="border-t border-line">
                  <td className="py-1 capitalize">{c.table}</td>
                  <td className="py-1">{c.added}</td>
                  <td className="py-1">{c.updated}</td>
                  <td className="py-1 text-muted">{c.unchanged}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="text-xs text-muted">
            <b>Merge</b> keeps the newest version of everything from both computers. <b>Replace</b> discards this
            computer's progress and uses the file's. A backup is saved before either.
          </p>
        </Modal>
      )}
    </Section>
  );
}

export default function SettingsPage() {
  const info = useAppInfo();
  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Settings</h1>
      </header>
      <div className="max-w-3xl px-6 py-5">
        {info.data && !info.data.pdfiumAvailable && (
          <div className="mb-4 flex items-center gap-2 rounded-md bg-bad/10 px-3 py-2 text-sm text-bad">
            <XCircle className="size-4" /> The PDF engine (pdfium) couldn't be loaded, so books can't be indexed. Run
            scripts/fetch-pdfium.sh and restart.
          </div>
        )}
        {info.data?.pdfiumAvailable && (
          <div className="mb-4 flex items-center gap-2 text-xs text-muted">
            <CheckCircle2 className="size-3.5 text-good" /> PDF engine ready
          </div>
        )}
        <ProvidersSection />
        <RolesSection />
        <DataSection />
      </div>
    </div>
  );
}
