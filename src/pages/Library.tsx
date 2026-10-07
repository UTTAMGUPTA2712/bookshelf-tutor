import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FilePlus2, Pencil, RefreshCw, Search, Trash2, Upload } from "lucide-react";
import { api, BOOK_STATUSES, errorMessage, type AddResult, type Book, type BookStatus } from "../ipc";
import { useAppInfo } from "../App";
import { Badge, Button, Empty, ErrorNote, Field, inputClass, Modal, Spinner } from "../ui";

const statusTone: Record<BookStatus, "accent" | "good" | "neutral" | "warn"> = {
  "start-here": "accent",
  core: "good",
  reference: "neutral",
  later: "neutral",
  outdated: "warn",
};

const statusOrder: BookStatus[] = ["start-here", "core", "reference", "later", "outdated"];

export function useBooks() {
  return useQuery({ queryKey: ["books"], queryFn: api.listBooks });
}

function BookCard({ book, onEdit }: { book: Book; onEdit: () => void }) {
  const navigate = useNavigate();
  const pct = book.progress && book.pageCount ? Math.min(100, Math.round((book.progress.lastPage / book.pageCount) * 100)) : 0;
  const missing = !book.path;
  return (
    <div
      className={`group relative flex cursor-pointer flex-col rounded-lg border border-line bg-panel p-3 transition hover:border-accent ${missing ? "opacity-60" : ""}`}
      onClick={() => !missing && navigate(`/book/${book.fingerprint}`)}
      title={missing ? "This book's file isn't on this computer" : book.fileName}
    >
      <div className="mb-1 flex items-start justify-between gap-2">
        <h3 className="line-clamp-2 text-sm leading-snug font-semibold">{book.title}</h3>
        <button
          className="shrink-0 rounded p-1 text-muted opacity-0 group-hover:opacity-100 hover:bg-sunken"
          onClick={(e) => {
            e.stopPropagation();
            onEdit();
          }}
          aria-label="Edit book"
        >
          <Pencil className="size-3.5" />
        </button>
      </div>
      {book.author && <p className="mb-2 line-clamp-1 text-xs text-muted">{book.author}</p>}
      <div className="mt-auto flex flex-wrap items-center gap-1.5">
        <Badge tone={statusTone[book.status] ?? "neutral"}>
          {BOOK_STATUSES.find((s) => s.id === book.status)?.label ?? book.status}
        </Badge>
        {book.level && <Badge>{book.level}</Badge>}
        {missing && <Badge tone="warn">not on this PC</Badge>}
        {book.pageCount && <span className="ml-auto text-[11px] text-muted">{book.pageCount} pages</span>}
      </div>
      {book.progress && (
        <div className="mt-2 h-1 overflow-hidden rounded bg-sunken" title={`Page ${book.progress.lastPage}`}>
          <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
        </div>
      )}
    </div>
  );
}

function EditBook({ book, onClose }: { book: Book; onClose: () => void }) {
  const qc = useQueryClient();
  const info = useAppInfo();
  const [form, setForm] = useState({
    title: book.title,
    author: book.author ?? "",
    track: book.track,
    status: book.status,
    level: book.level ?? "",
  });
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [deleteFile, setDeleteFile] = useState(false);
  const save = useMutation({
    mutationFn: () => api.updateBook(book.fingerprint, form),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["books"] });
      onClose();
    },
  });
  const remove = useMutation({
    mutationFn: () => api.removeBook(book.fingerprint, deleteFile, false),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["books"] });
      onClose();
    },
  });
  const set = (k: keyof typeof form) => (e: { target: { value: string } }) => setForm({ ...form, [k]: e.target.value });

  return (
    <Modal
      title="Edit book"
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" className="mr-auto text-bad" onClick={() => setConfirmRemove(true)}>
            <Trash2 className="size-4" /> Remove
          </Button>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={save.isPending} onClick={() => save.mutate()}>
            Save
          </Button>
        </>
      }
    >
      <Field label="Title">
        <input className={inputClass} value={form.title} onChange={set("title")} />
      </Field>
      <Field label="Author">
        <input className={inputClass} value={form.author} onChange={set("author")} />
      </Field>
      <div className="grid grid-cols-3 gap-3">
        <Field label="Track">
          <select className={inputClass} value={form.track} onChange={set("track")}>
            {info.data?.tracks.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Status">
          <select className={inputClass} value={form.status} onChange={set("status")}>
            {BOOK_STATUSES.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Level">
          <select className={inputClass} value={form.level} onChange={set("level")}>
            <option value="">—</option>
            <option value="beginner">Beginner</option>
            <option value="intermediate">Intermediate</option>
            <option value="advanced">Advanced</option>
          </select>
        </Field>
      </div>
      <p className="text-xs break-all text-muted">{book.path ?? "File not found on this computer."}</p>
      {confirmRemove && (
        <div className="mt-4 rounded-md border border-bad/30 bg-bad/5 p-3 text-sm">
          <p className="mb-2">Remove this book from the library? Your quiz history is kept.</p>
          <label className="mb-3 flex items-center gap-2 text-xs">
            <input type="checkbox" checked={deleteFile} onChange={(e) => setDeleteFile(e.target.checked)} />
            Also delete the PDF file (only if it's inside the library folder)
          </label>
          <Button variant="danger" busy={remove.isPending} onClick={() => remove.mutate()}>
            Remove book
          </Button>
        </div>
      )}
      <ErrorNote error={save.error ? errorMessage(save.error) : remove.error ? errorMessage(remove.error) : null} />
    </Modal>
  );
}

function AddReport({ result, onClose }: { result: AddResult; onClose: () => void }) {
  return (
    <Modal title="Books added" onClose={onClose} footer={<Button onClick={onClose}>OK</Button>}>
      <p className="mb-2 text-sm">
        Added <b>{result.added.length}</b> book{result.added.length === 1 ? "" : "s"}.
      </p>
      {result.duplicates.length > 0 && (
        <div className="mb-2 text-sm">
          <p className="font-medium">Already in your library:</p>
          <ul className="list-disc pl-5 text-muted">
            {result.duplicates.map((d) => (
              <li key={d.file}>
                {d.file} → {d.existingTitle}
              </li>
            ))}
          </ul>
        </div>
      )}
      {result.errors.length > 0 && <ErrorNote error={result.errors.join("\n")} />}
    </Modal>
  );
}

export default function LibraryPage() {
  const qc = useQueryClient();
  const info = useAppInfo();
  const books = useBooks();
  const [query, setQuery] = useState("");
  const [hideOutdated, setHideOutdated] = useState(true);
  const [editing, setEditing] = useState<Book | null>(null);
  const [addResult, setAddResult] = useState<AddResult | null>(null);
  const [dragging, setDragging] = useState(false);
  const [addMode, setAddMode] = useState<"copy" | "link">("copy");

  const rescan = useMutation({
    mutationFn: api.scanLibrary,
    onSuccess: (data) => qc.setQueryData(["books"], data),
  });
  const add = useMutation({
    mutationFn: (paths: string[]) => api.addBooks(paths, addMode),
    onSuccess: (r) => {
      setAddResult(r);
      qc.invalidateQueries({ queryKey: ["books"] });
    },
  });

  async function pickFiles() {
    const files = await open({ multiple: true, filters: [{ name: "PDF books", extensions: ["pdf"] }] });
    if (files && files.length) add.mutate(files as string[]);
  }

  // Drag & drop PDFs onto the window
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") setDragging(true);
        else if (p.type === "leave") setDragging(false);
        else if (p.type === "drop") {
          setDragging(false);
          const pdfs = p.paths.filter((x) => x.toLowerCase().endsWith(".pdf"));
          if (pdfs.length) add.mutate(pdfs);
        }
      })
      .then((u) => (unlisten = u));
    return () => unlisten?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [addMode]);

  const grouped = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = (books.data ?? []).filter(
      (b) =>
        (!hideOutdated || b.status !== "outdated") &&
        (!q || b.title.toLowerCase().includes(q) || (b.author ?? "").toLowerCase().includes(q)),
    );
    const tracks = info.data?.tracks ?? [];
    return tracks
      .map((t) => ({
        track: t,
        books: list
          .filter((b) => b.track === t.id || (!tracks.some((x) => x.id === b.track) && t.id === "uncategorized"))
          .sort((a, b) => statusOrder.indexOf(a.status) - statusOrder.indexOf(b.status) || a.title.localeCompare(b.title)),
      }))
      .filter((g) => g.books.length > 0);
  }, [books.data, info.data, query, hideOutdated]);

  const reading = (books.data ?? []).filter((b) => b.progress?.status === "reading" && b.path);

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex flex-wrap items-center gap-2 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="mr-4 text-lg font-semibold">Library</h1>
        <div className="relative">
          <Search className="absolute top-2 left-2 size-4 text-muted" />
          <input
            className={`${inputClass} w-64 pl-8`}
            placeholder="Search title or author"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <label className="flex items-center gap-1.5 text-xs text-muted">
          <input type="checkbox" checked={hideOutdated} onChange={(e) => setHideOutdated(e.target.checked)} />
          Hide outdated
        </label>
        <div className="ml-auto flex items-center gap-2">
          <select
            className={`${inputClass} w-auto`}
            value={addMode}
            onChange={(e) => setAddMode(e.target.value as "copy" | "link")}
            title="Copy: the PDF is copied into your library folder. Link: the app remembers where the file is."
          >
            <option value="copy">Copy into library</option>
            <option value="link">Link in place</option>
          </select>
          <Button variant="primary" busy={add.isPending} onClick={pickFiles}>
            <FilePlus2 className="size-4" /> Add books
          </Button>
          <Button busy={rescan.isPending} onClick={() => rescan.mutate()} title="Look for new or removed PDFs">
            <RefreshCw className="size-4" /> Rescan
          </Button>
        </div>
      </header>

      <div className="px-6 py-4">
        <ErrorNote error={rescan.error ? errorMessage(rescan.error) : add.error ? errorMessage(add.error) : null} />
        {books.isLoading && <Spinner />}
        {books.data?.length === 0 && (
          <Empty title="No books yet">Click “Add books” or drop PDF files onto this window.</Empty>
        )}

        {reading.length > 0 && !query && (
          <section className="mb-6">
            <h2 className="mb-2 text-xs font-semibold tracking-wide text-muted uppercase">Continue reading</h2>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
              {reading.map((b) => (
                <BookCard key={b.fingerprint} book={b} onEdit={() => setEditing(b)} />
              ))}
            </div>
          </section>
        )}

        {grouped.map((g) => (
          <section key={g.track.id} className="mb-6">
            <h2 className="mb-2 text-xs font-semibold tracking-wide text-muted uppercase">
              {g.track.label} <span className="font-normal">· {g.books.length}</span>
            </h2>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
              {g.books.map((b) => (
                <BookCard key={b.fingerprint} book={b} onEdit={() => setEditing(b)} />
              ))}
            </div>
          </section>
        ))}
      </div>

      {dragging && (
        <div className="pointer-events-none fixed inset-0 z-40 flex items-center justify-center bg-accent/10">
          <div className="flex items-center gap-2 rounded-lg border-2 border-dashed border-accent bg-panel px-6 py-4 text-accent">
            <Upload className="size-5" /> Drop PDFs to add them ({addMode === "copy" ? "copied into library" : "linked"})
          </div>
        </div>
      )}
      {editing && <EditBook book={editing} onClose={() => setEditing(null)} />}
      {addResult && <AddReport result={addResult} onClose={() => setAddResult(null)} />}
    </div>
  );
}
