import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Photo, ViewerState } from "./types";

const previewCache = new Map<string, string>();
const previewRequests = new Map<string, Promise<string>>();
export function clearPreviewCache() { previewCache.clear(); }
function LargePhoto({ photo, label }: { photo: Photo; label: string }) {
  const cacheKey = `${photo.id}:${photo.absolute_path}:${photo.sha256}`;
  const [source, setSource] = useState<string | null>(previewCache.get(cacheKey) ?? null);
  const [error, setError] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  useEffect(() => {
    let cancelled = false;
    let pending = previewRequests.get(cacheKey);
    if (!previewCache.has(cacheKey)) {
      if (!pending) {
        pending = invoke<string>("get_photo_preview", { id: photo.id }).then(url => {
          if (previewCache.size >= 8) previewCache.delete(previewCache.keys().next().value!);
          previewCache.set(cacheKey, url);
          return url;
        }).finally(() => previewRequests.delete(cacheKey));
        previewRequests.set(cacheKey, pending);
      }
      pending.then(url => { if (!cancelled) setSource(url); })
        .catch(err => { if (!cancelled) setError(String(err)); });
    }
    return () => { cancelled = true; };
  }, [cacheKey, photo.id]);
  return <section className="flex flex-col min-w-0 min-h-0 bg-white rounded-xl border border-gray-200 overflow-hidden" aria-label={`${label}: ${photo.filename}`}>
    <div className="flex items-center justify-between gap-3 p-3 bg-gray-50 border-b border-gray-200">
      <div className="min-w-0"><p className="text-xs font-semibold text-blue-700">{label}</p><p className="text-sm truncate" title={photo.absolute_path}>{photo.filename}</p></div>
      <button className="ui-button ui-button--secondary text-xs flex-shrink-0" aria-pressed={zoom > 1} onClick={() => setZoom(zoom === 1 ? 2 : 1)}>{zoom === 1 ? "Zoom in" : "Fit photo"}</button>
    </div>
    <div className="flex-1 min-h-0 overflow-auto p-2 bg-gray-100" aria-busy={!source && !error}>
      {error ? <div role="alert" className="p-6 text-red-700"><p>Could not open this photo.</p><p className="text-xs mt-2">{error}</p></div> : source ?
        <div style={{ width: `${zoom * 100}%`, height: `${zoom * 100}%` }}><img src={source} alt={photo.filename} className="w-full h-full object-contain" /></div> :
        <p role="status" className="p-8 text-gray-600 text-center">Loading large preview…</p>}
    </div>
    <p className="px-3 py-2 text-xs text-gray-600 truncate" title={photo.absolute_path}>{photo.width && photo.height ? `${photo.width} × ${photo.height} · ` : ""}{(photo.file_size / 1048576).toFixed(2)} MB · {photo.absolute_path}</p>
  </section>;
}

export default function PhotoViewer({ viewer, busy, message, onViewBin, onClose, onSelect, onKeep, onBin, onRecover, onPurge }: {
  viewer: ViewerState;
  busy: boolean;
  message?: string;
  onViewBin: () => void;
  onClose: () => void;
  onSelect: (id: string) => void;
  onKeep: (photo: Photo, group: Photo[]) => void;
  onBin: (ids: string[]) => void;
  onRecover: (ids: string[]) => void;
  onPurge: (ids: string[]) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    const element = dialog.current!;
    element.showModal();
    return () => { element.close(); if (trigger?.isConnected) trigger.focus(); };
  }, []);
  useLayoutEffect(() => {
    const element = dialog.current;
    const active = document.activeElement;
    if (element?.open && (!element.contains(active) || (active instanceof HTMLButtonElement && active.disabled))) {
      element.querySelector<HTMLButtonElement>('[data-viewer-next]')?.focus();
    }
  }, [viewer.selectedId]);
  const selected = viewer.photos.find(p => p.id === viewer.selectedId) ?? viewer.photos[0];
  const keeper = viewer.photos.find(p => p.id === viewer.keeperId);
  if (!selected) return null;
  const index = viewer.photos.findIndex(p => p.id === selected.id);
  const next = (delta: number) => onSelect(viewer.photos[(index + delta + viewer.photos.length) % viewer.photos.length].id);
  const compare = keeper && keeper.id !== selected.id;
  return <dialog ref={dialog} className="photo-dialog bg-gray-50 text-gray-900 p-0 rounded-2xl shadow-2xl" aria-labelledby="viewer-title" onCancel={event => { event.preventDefault(); onClose(); }} onKeyDown={event => {
    if (event.key === "ArrowLeft") { event.preventDefault(); next(-1); }
    if (event.key === "ArrowRight") { event.preventDefault(); next(1); }
  }}>
    <div className="flex flex-col h-full p-4 gap-4">
      <header className="flex justify-between items-start gap-4">
        <div><h2 id="viewer-title" className="page-title">{viewer.inBin ? "Review photo in Bin" : compare ? "Compare photos" : "Photo viewer"}</h2><p className="text-gray-600 text-sm mt-1">{viewer.inBin ? "Recover returns the photo to its original folder." : "Inspect both photos, choose a keeper, then move unwanted photos to Bin."}</p></div>
        <button onClick={onClose} className="ui-button ui-button--secondary">Close</button>
      </header>
      {message && <p role="status" className="text-sm text-blue-700">{message}</p>}
      <div className={`flex-1 min-h-0 grid gap-4 ${compare ? "grid-cols-1 sm:grid-cols-2" : "grid-cols-1"}`}>
        {compare && <LargePhoto key={`keeper:${keeper.id}:${keeper.sha256}`} photo={keeper} label={keeper.is_kept ? "Kept photo" : "Suggested keeper"} />}
        <LargePhoto key={`${selected.id}:${selected.absolute_path}:${selected.sha256}`} photo={selected} label={viewer.inBin ? "In Bin" : selected.is_kept ? "Kept photo" : "Selected photo"} />
      </div>
      <footer className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3"><button disabled={viewer.photos.length < 2} onClick={() => next(-1)} className="ui-button ui-button--secondary">Previous</button><span className="text-sm text-gray-600" aria-live="polite">{index + 1} / {viewer.photos.length}</span><button data-viewer-next disabled={viewer.photos.length < 2} onClick={() => next(1)} className="ui-button ui-button--secondary">Next</button></div>
        <div className="flex flex-wrap gap-3"><button onClick={onViewBin} className="ui-button ui-button--secondary">View Bin</button>{viewer.inBin ? <>
          <button disabled={busy} onClick={() => onRecover([selected.id])} className="ui-button ui-button--primary">Recover photo</button>
          <button disabled={busy} onClick={() => onPurge([selected.id])} className="ui-button ui-button--danger">Delete permanently…</button>
        </> : <>
          <button disabled={busy || selected.is_kept} onClick={() => onKeep(selected, keeper ? viewer.photos : [selected])} className="ui-button ui-button--keep">{selected.is_kept ? "Marked Keep" : "Keep this photo"}</button>
          <button disabled={busy || selected.is_kept} title={selected.is_kept ? "Choose another keeper before moving this photo to Bin" : "Recoverable from Bin"} onClick={() => onBin([selected.id])} className="ui-button ui-button--danger">Move to Bin</button>
        </>}</div>
      </footer>
    </div>
  </dialog>;
}
