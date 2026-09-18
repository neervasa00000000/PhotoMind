import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, confirm as confirmAction } from "@tauri-apps/plugin-dialog";
import { VirtuosoGrid } from "react-virtuoso";
import PhotoViewer, { clearPreviewCache } from "./PhotoViewer";
import { planCleanup } from "./cleanup";
import type { Photo, BinPhoto, FileActionReport, ViewerState, DuplicateGroup } from "./types";
import { FolderOpen, Layers, Image as ImageIcon, Settings, Cpu, LayoutDashboard, Trash2 } from "lucide-react";

interface OllamaModel {
  name: string;
  is_vision: boolean;
}

interface BlurAnalysis {
  blurred: boolean;
  level: number;
  cause?: string | null;
}

interface FaceDetail {
  eye_state: "OPEN" | "PARTIAL" | "CLOSED" | "UNCLEAR";
  looking_at_camera: boolean;
  emotion?: string | null;
  quality: number;
}

interface AnalysisResult {
  scene_type: string;
  people: any;
  subject: any;
  composition: any;
  aesthetic_score: number;
  blur?: BlurAnalysis | null;
  faces?: FaceDetail[] | null;
  suggestions?: string[] | null;
  problems: string[];
  summary: string;
}

interface AiAnalysis {
  photo: Photo;
  model: string;
  analysis_json: string;
  analyzed_at: string | null;
}

interface MomentGroup {
  id: string;
  start_time: string | null;
  end_time: string | null;
  photo_count: number;
}

interface Recommendation {
  photo_id: string;
  moment_id: string | null;
  decision: string;
  confidence: number;
  reasoning: string;
  compared_to: string | null;
  reason_codes?: string | null;
  best_of_group?: number | null;
}

interface PhotoFaceSummary {
  photo_id: string;
  face_count: number;
  closed_eye_warning: boolean;
  uncertain_eyes: boolean;
  average_face_sharpness: number | null;
  average_smile: number | null;
}

function formatReasoning(raw: string): string {
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) return parsed.join(" · ");
    if (parsed && Array.isArray(parsed.messages)) return parsed.messages.join(" · ");
  } catch { /* legacy text */ }
  return raw;
}

interface MomentGroupWithPhotos {
  moment: MomentGroup;
  photos: Photo[];
  recommendations: Recommendation[];
}

interface DashboardStats {
  total_photos: number;
  total_bytes: number;
  exact_duplicates: number;
  exact_duplicates_bytes: number;
  recommended_removals: number;
  recommended_removals_bytes: number;
  needs_review: number;
}

const thumbnailCache = new Map<string, string>();

function PhotoThumb({
  photoId,
  version,
  filename,
  className,
}: {
  photoId: string;
  version?: string | null;
  filename: string;
  className?: string;
}) {
  const cacheKey = `${photoId}:${version ?? ""}`;
  const [src, setSrc] = useState<string | null>(thumbnailCache.get(cacheKey) ?? null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const cached = thumbnailCache.get(cacheKey) ?? null;
    setSrc(cached);
    setFailed(false);
    if (cached) return;
    let cancelled = false;
    invoke<string>("get_thumbnail_data_url", { id: photoId })
      .then((url) => {
        if (cancelled) return;
        if (thumbnailCache.size >= 500) {
          const oldest = thumbnailCache.keys().next().value;
          if (oldest !== undefined) thumbnailCache.delete(oldest);
        }
        thumbnailCache.set(cacheKey, url);
        setSrc(url);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [photoId, cacheKey]);

  if (failed) {
    return (
      <div className={`${className ?? ""} flex items-center justify-center bg-gray-100 text-gray-400 text-xs p-2 text-center`}>
        Could not load
      </div>
    );
  }

  if (!src) {
    return <div className={`${className ?? ""} bg-gray-200 animate-pulse`} />;
  }

  return <img src={src} alt={filename} className={className} />;
}

function App() {
  const [activeTab, setActiveTab] = useState<'dashboard' | 'library' | 'duplicates' | 'moments' | 'settings' | 'bin'>('dashboard');
  const [photos, setPhotos] = useState<Photo[]>([]);
  const photoOffset = useRef(0);
  const photoLoading = useRef(false);
  const photoGeneration = useRef(0);
  const hasMorePhotos = useRef(true);
  const [binPhotos, setBinPhotos] = useState<BinPhoto[]>([]);
  const [viewer, setViewer] = useState<ViewerState | null>(null);
  const [fileBusy, setFileBusy] = useState(false);
  const fileBusyRef = useRef(false);
  const [notice, setNotice] = useState<{ text: string; ids?: string[]; error?: boolean } | null>(null);
  const [duplicates, setDuplicates] = useState<DuplicateGroup[]>([]);
  const [recommendations, setRecommendations] = useState<Map<string, Recommendation>>(new Map());
  const [moments, setMoments] = useState<MomentGroupWithPhotos[]>([]);
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [scanning, setScanning] = useState(false);
  const [scanPaused, setScanPaused] = useState(false);
  const [scanProgress, setScanProgress] = useState<{processed: number, total: number, current_file: string} | null>(null);
  
  const [ollamaModels, setOllamaModels] = useState<OllamaModel[] | null>(null);
  const [ollamaError, setOllamaError] = useState<string | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | null>(null);
  const [localProcessingOnly, setLocalProcessingOnly] = useState(true);
  
  const [testAnalysis, setTestAnalysis] = useState<AnalysisResult | null>(null);
  const [testAnalysisPhoto, setTestAnalysisPhoto] = useState<Photo | null>(null);
  const [isAnalyzing, setIsAnalyzing] = useState(false);
  const [aiAnalyses, setAiAnalyses] = useState<Map<string, AnalysisResult>>(new Map());
  const [faceSummaries, setFaceSummaries] = useState<Map<string, PhotoFaceSummary>>(new Map());
  const [aiRunning, setAiRunning] = useState(false);
  const [aiProgress, setAiProgress] = useState<{processed: number; total: number; model: string; current_file: string} | null>(null);

  useEffect(() => {
    let lastPhotoRefresh = 0;
    let loadingScanPhotos = false;
    const unlistenProgress = listen('scan-progress', (event: any) => {
      setScanProgress(event.payload);
      setScanning(true);
      // Show useful previews as work completes, without repeatedly recomputing global groups.
      if (!loadingScanPhotos && event.payload.processed > 0 && Date.now() - lastPhotoRefresh > 1500) {
        lastPhotoRefresh = Date.now();
        loadingScanPhotos = true;
        const generation = photoGeneration.current;
        invoke<Photo[]>("get_photos", { offset: 0, limit: 200 }).then(page => {
          if (generation !== photoGeneration.current) return;
          setPhotos(page);
          photoOffset.current = page.length;
          hasMorePhotos.current = page.length === 200;
        }).catch(console.error).finally(() => { loadingScanPhotos = false; });
      }
    });
    
    const unlistenLocalResults = listen('local-results-ready', async () => {
      // Global visual grouping is fetched at completion; don't repeat its quadratic work per batch.
      try {
        const generation = photoGeneration.current;
        const [groups, recs, currentStats] = await Promise.all([
          invoke<MomentGroupWithPhotos[]>("get_moment_groups"), invoke<Recommendation[]>("get_recommendations"), invoke<DashboardStats>("get_stats")
        ]);
        if (generation !== photoGeneration.current) return;
        setMoments(groups); setRecommendations(new Map(recs.map(r => [r.photo_id, r]))); setStats(currentStats);
      } catch (error) { console.error("Could not update local results", error); }
    });

    const unlistenComplete = listen<{ processed: number; failed?: number; phase?: string }>('scan-complete', async event => {
      setScanning(false);
      setScanPaused(false);
      setScanProgress(null);
      await refreshData();
      if (event.payload.failed) setNotice({ text: `${event.payload.processed} photos processed; ${event.payload.failed} files could not be analyzed. Other photos remain available.` });
      setActiveTab("library");
    });

    const unlistenError = listen<{ message: string }>('scan-error', async (event) => {
      setScanning(false);
      setScanPaused(false);
      setScanProgress(null);
      alert(event.payload.message);
      await refreshData();
    });

    const unlistenAiProgress = listen<{ processed: number; total: number; model: string; current_file: string }>('ai-progress', (event) => {
      setAiProgress(event.payload);
      setAiRunning(true);
    });

    const unlistenAiComplete = listen<{ processed: number; total: number; failed: number; cancelled?: boolean }>('ai-complete', async (event) => {
      setAiProgress(null);
      setAiRunning(false);
      await refreshData();
      const p = event.payload;
      if (p.total > 0 && !p.cancelled) {
        setNotice({ text: `AI analysis complete: ${p.processed} photo(s) analyzed${p.failed ? `, ${p.failed} failed` : ""}.` });
      }
    });

    const unlistenAiError = listen<{ message: string }>('ai-error', async (event) => {
      setAiProgress(null);
      setAiRunning(false);
      setNotice({ text: event.payload.message, error: true });
      await refreshData();
    });

    // Load data on initial startup
    refreshData();

    return () => {
      unlistenProgress.then(f => f());
      unlistenComplete.then(f => f());
      unlistenLocalResults.then(f => f());
      unlistenError.then(f => f());
      unlistenAiProgress.then(f => f());
      unlistenAiComplete.then(f => f());
      unlistenAiError.then(f => f());
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let refreshing = false;
    const syncBin = async () => {
      if (fileBusyRef.current || document.hidden || refreshing) return;
      refreshing = true;
      try {
        const changed = await invoke<boolean>("refresh_library");
        if (cancelled) return;
        if (changed) {
          thumbnailCache.clear(); clearPreviewCache(); setViewer(null);
          await refreshData();
          return;
        }
        const current = await invoke<BinPhoto[]>("get_bin");
        if (cancelled) return;
        setBinPhotos(current);
        setViewer(previous => {
          if (!previous?.inBin) return previous;
          const remaining = previous.photos.filter(p => current.some(entry => entry.photo.id === p.id));
          if (!remaining.length) return null;
          return { ...previous, photos: remaining, selectedId: remaining.some(p => p.id === previous.selectedId) ? previous.selectedId : remaining[0].id };
        });
      } catch (error) { console.error("Could not sync library", error); }
      finally { refreshing = false; }
    };
    window.addEventListener("focus", syncBin);
    document.addEventListener("visibilitychange", syncBin);
    syncBin();
    const timer = window.setInterval(syncBin, activeTab === "bin" ? 5000 : 30000);
    return () => { cancelled = true; window.removeEventListener("focus", syncBin); document.removeEventListener("visibilitychange", syncBin); if (timer) window.clearInterval(timer); };
  }, [activeTab]);

  const loadMorePhotos = async () => {
    if (photoLoading.current || !hasMorePhotos.current) return;
    photoLoading.current = true;
    const generation = photoGeneration.current;
    try {
      const page = await invoke<Photo[]>("get_photos", { offset: photoOffset.current, limit: 200 });
      if (generation !== photoGeneration.current) return;
      photoOffset.current += page.length;
      hasMorePhotos.current = page.length === 200;
      setPhotos(previous => [...previous, ...page]);
    } catch (err) {
      console.error("Failed to load library page", err);
      alert("Could not load more photos: " + String(err));
    } finally {
      if (generation === photoGeneration.current) photoLoading.current = false;
    }
  };

  const refreshData = async () => {
    const generation = ++photoGeneration.current;
    photoLoading.current = true;
    try {
      const fetchedStats = await invoke<DashboardStats>("get_stats");
      if (generation !== photoGeneration.current) return;
      setStats(fetchedStats);
      
      const fetchedPhotos = await invoke<Photo[]>("get_photos", { offset: 0, limit: 200 });
      if (generation !== photoGeneration.current) return;
      photoOffset.current = fetchedPhotos.length;
      hasMorePhotos.current = fetchedPhotos.length === 200;
      setPhotos(fetchedPhotos);
      
      const fetchedDuplicates = await invoke<DuplicateGroup[]>("get_duplicate_groups");
      if (generation !== photoGeneration.current) return;
      setDuplicates(fetchedDuplicates);

      const fetchedMoments = await invoke<MomentGroupWithPhotos[]>("get_moment_groups");
      if (generation !== photoGeneration.current) return;
      setMoments(fetchedMoments);
      const recs = await invoke<Recommendation[]>("get_recommendations");
      if (generation !== photoGeneration.current) return;
      setRecommendations(new Map(recs.map(r => [r.photo_id, r])));
      const bin = await invoke<BinPhoto[]>("get_bin");
      if (generation !== photoGeneration.current) return;
      setBinPhotos(bin);
      const fetchedAi = await invoke<AiAnalysis[]>("get_ai_analyses");
      if (generation !== photoGeneration.current) return;
      const aiMap = new Map<string, AnalysisResult>();
      for (const entry of fetchedAi) {
        try { aiMap.set(entry.photo.id, JSON.parse(entry.analysis_json)); } catch (_) { /* ignore malformed */ }
      }
      setAiAnalyses(aiMap);
      const faceRows = await invoke<PhotoFaceSummary[]>("get_photo_face_summaries");
      if (generation !== photoGeneration.current) return;
      setFaceSummaries(new Map(faceRows.map(row => [row.photo_id, row])));
    } catch (err) {
      console.error("Failed to load data", err);
      alert("Could not refresh library: " + String(err));
    } finally {
      if (generation === photoGeneration.current) photoLoading.current = false;
    }
  };

  const checkOllamaStatus = async () => {
    try {
      setOllamaError(null);
      const models = await invoke<OllamaModel[]>("check_ollama");
      setOllamaModels(models);
      if (models.length > 0) {
        const visionModel = models.find(m => m.is_vision);
        if (visionModel) {
          setSelectedModel(visionModel.name);
        } else {
          setSelectedModel(null);
        }
      }
    } catch (err: any) {
      setOllamaError(err.toString());
      setOllamaModels([]);
    }
  };

  const testAnalyze = async (photo: Photo) => {
    if (!selectedModel) return;
    setIsAnalyzing(true);
    setTestAnalysis(null);
    setTestAnalysisPhoto(photo);
    try {
      const result = await invoke<AnalysisResult>("test_analyze_photo", {
        model: selectedModel,
        photoPath: photo.absolute_path
      });
      setTestAnalysis(result);
      setAiAnalyses(previous => new Map(previous).set(photo.id, result));
    } catch (err: any) {
      console.error(err);
      alert("Analysis failed: " + err.toString());
    } finally {
      setIsAnalyzing(false);
    }
  };

  const openAiDetails = (photo: Photo) => {
    const cached = aiAnalyses.get(photo.id);
    if (cached) {
      setTestAnalysis(cached);
      setTestAnalysisPhoto(photo);
    } else {
      testAnalyze(photo);
    }
  };

  const handleUserOverride = async (photoId: string, currentRecommendation: string, newDecision: string) => {
    try {
      await invoke("log_user_decision", {
        photoId,
        recommended: currentRecommendation,
        actual: newDecision
      });
      await refreshData();
    } catch (err) {
      console.error("Failed to save decision override", err);
    }
  };

  const removeFromViewer = (ids: string[]) => {
    setViewer(previous => {
      if (!previous) return null;
      const remaining = previous.photos.filter(p => !ids.includes(p.id));
      if (!remaining.length) return null;
      return { ...previous, photos: remaining, selectedId: remaining.some(p => p.id === previous.selectedId) ? previous.selectedId : remaining.find(p => p.id !== previous.keeperId)?.id ?? remaining[0].id };
    });
  };

  const fileAction = async (command: string, ids: string[], verb: string, keeper?: { photo: Photo; group: Photo[] } | { photo: Photo; group: Photo[] }[]) => {
    if (!ids.length || fileBusyRef.current) return;
    if (command === "move_to_trash" && ids.length > 1) {
      const approved = await confirmAction(`Move ${ids.length} selected photos to Bin? They remain recoverable until permanently deleted.`, { title: "Review bulk removal", kind: "warning" });
      if (!approved || fileBusyRef.current) return;
    }
    fileBusyRef.current = true;
    setFileBusy(true);
    try {
      if (keeper) {
        for (const entry of Array.isArray(keeper) ? keeper : [keeper]) {
          await invoke("choose_keeper", { photoId: entry.photo.id, groupIds: entry.group.map(p => p.id) });
        }
      }
      const report = await invoke<FileActionReport>(command, { photoIds: ids });
      removeFromViewer(report.photo_ids);
      setNotice({ text: `${report.photo_ids.length} photo(s) ${verb}.${report.errors.length ? " " + report.errors.join(" ") : ""}`, ids: command === "move_to_trash" ? report.photo_ids : undefined, error: report.errors.length > 0 });
      await refreshData();
      if (activeTab === 'bin' && command !== 'move_to_trash') requestAnimationFrame(() => {
        if (!document.querySelector('dialog[open]')) document.getElementById('bin-title')?.focus();
      });
    } catch (err) {
      setNotice({ text: String(err), error: true });
      await refreshData();
    } finally {
      fileBusyRef.current = false;
      setFileBusy(false);
    }
  };

  const trashPhotos = (ids: string[]) => fileAction("move_to_trash", ids, "moved to Bin; recover them any time before permanently deleting");
  const recoverPhotos = (ids: string[]) => fileAction("recover_photos", ids, "recovered to their original folders");
  const purgePhotos = async (ids: string[]) => {
    if (!ids.length || fileBusyRef.current) return;
    const approved = await confirmAction(`Permanently delete ${ids.length} photo(s) from Bin? This cannot be undone.`, { title: "Delete permanently", kind: "warning" });
    if (approved) await fileAction("delete_permanently", ids, "permanently deleted");
  };
  const keepPhoto = async (photo: Photo, group: Photo[]) => {
    if (fileBusyRef.current) return;
    fileBusyRef.current = true;
    setFileBusy(true);
    try {
      await invoke("choose_keeper", { photoId: photo.id, groupIds: group.map(p => p.id) });
      setViewer(previous => previous ? { ...previous, keeperId: previous.keeperId ? photo.id : undefined, selectedId: previous.keeperId && previous.selectedId === photo.id ? previous.photos.find(p => p.id !== photo.id)?.id ?? photo.id : previous.selectedId, photos: previous.photos.map(p => ({ ...p, is_kept: group.some(g => g.id === p.id) ? p.id === photo.id : p.is_kept })) } : null);
      setNotice({ text: `${photo.filename} is marked Keep and protected from cleanup.` });
      await refreshData();
    } catch (error) { setNotice({ text: String(error), error: true }); }
    finally { fileBusyRef.current = false; setFileBusy(false); }
  };
  const openViewer = (group: Photo[], selected: Photo, inBin = false, keeperId?: string) => {
    setNotice(null);
    setViewer({ photos: group, selectedId: selected.id, inBin, keeperId });
  };

  const exactCleanup = planCleanup(duplicates, "exact");
  const visualCleanup = planCleanup(duplicates, "similar");
  const allCleanup = planCleanup(duplicates);
  const recommendedPhotos = moments.flatMap(group => group.recommendations.filter(rec => rec.decision === "REMOVE").flatMap(rec => group.photos.filter(p => p.id === rec.photo_id && !p.is_kept)));
  const cleanupBytes = allCleanup.bytes + [...new Map(recommendedPhotos.filter(p => !allCleanup.ids.includes(p.id)).map(p => [p.id, p])).values()].reduce((sum, p) => sum + p.file_size, 0);
  const runGroupCleanup = (kind: DuplicateGroup["kind"]) => {
    const plan = planCleanup(duplicates, kind);
    return fileAction("move_to_trash", plan.ids, "moved to Bin; suggested or chosen keepers are protected", plan.keepers);
  };

  const selectFolder = async () => {
    const selected = await open({
      directory: true,
      multiple: false,
    });
    if (selected && typeof selected === "string") {
      setScanning(true);
      setScanPaused(false);
      try {
        await invoke("start_scan", { path: selected });
      } catch (err) {
        console.error("Failed to start scan", err);
        alert("Could not start scan: " + String(err));
        setScanning(false);
      }
    }
  };

  const pauseScan = async () => {
    await invoke("pause_scan");
    setScanPaused(true);
  };

  const resumeScan = async () => {
    await invoke("resume_scan");
    setScanPaused(false);
  };

  const cancelScan = async () => {
    await invoke("cancel_scan");
    // Keep the scan locked until the backend acknowledges completion.
    setScanPaused(false);
  };

  return (
    <div className="flex flex-col h-screen bg-gray-50 text-gray-900 font-sans">
      <header className="flex-shrink-0 bg-white shadow-sm px-6 py-4 flex items-center justify-between z-10 border-b border-gray-100">
        <div className="flex items-center gap-3 min-w-0 flex-1">
          <div className="flex-shrink-0">
            <h1 className="text-2xl font-semibold text-gray-900 tracking-tight leading-none">PhotoMind</h1>
            <p className="text-xs text-blue-600 font-medium tracking-wide uppercase mt-1">100% LOCAL PRIVACY</p>
          </div>
          
          {(photos.length > 0 || binPhotos.length > 0 || scanning) && (
            <div className="flex flex-wrap bg-gray-100 p-1 rounded-lg">
              <button 
                onClick={() => setActiveTab('dashboard')}
                className={`px-4 py-2 rounded-md text-sm font-medium transition-colors flex items-center gap-2 ${activeTab === 'dashboard' ? 'bg-white shadow-sm text-gray-900' : 'text-gray-500 hover:text-gray-900'}`}
              >
                <LayoutDashboard size={16} /> Dashboard
              </button>
              <button 
                onClick={() => setActiveTab('library')}
                className={`px-4 py-2 rounded-md text-sm font-medium transition-colors flex items-center gap-2 ${activeTab === 'library' ? 'bg-white shadow-sm text-gray-900' : 'text-gray-500 hover:text-gray-900'}`}
              >
                <ImageIcon size={16} /> Library
              </button>
              <button 
                onClick={() => setActiveTab('duplicates')}
                className={`px-4 py-2 rounded-md text-sm font-medium transition-colors flex items-center gap-2 ${activeTab === 'duplicates' ? 'bg-white shadow-sm text-gray-900' : 'text-gray-500 hover:text-gray-900'}`}
              >
                <Layers size={16} /> Duplicates
                {duplicates.length > 0 && (
                  <span className="bg-blue-100 text-blue-700 py-0.5 px-2 rounded-full text-xs font-bold ml-1">
                    {duplicates.length}
                  </span>
                )}
              </button>
              <button 
                onClick={() => setActiveTab('moments')}
                className={`px-4 py-2 rounded-md text-sm font-medium transition-colors flex items-center gap-2 ${activeTab === 'moments' ? 'bg-white shadow-sm text-gray-900' : 'text-gray-500 hover:text-gray-900'}`}
              >
                <Layers size={16} /> Moments
                {moments.length > 0 && (
                  <span className="bg-purple-100 text-purple-700 py-0.5 px-2 rounded-full text-xs font-bold ml-1">
                    {moments.length}
                  </span>
                )}
              </button>
              <button onClick={() => setActiveTab('bin')} aria-current={activeTab === 'bin' ? 'page' : undefined} className={`px-3 py-2 rounded-md text-sm font-medium flex items-center gap-2 whitespace-nowrap ${activeTab === 'bin' ? 'bg-white shadow-sm' : 'text-gray-600 hover:text-gray-900'}`}><Trash2 size={16} /> Bin {binPhotos.length > 0 && <span>{binPhotos.length}</span>}</button>
              <button 
                onClick={() => { setActiveTab('settings'); checkOllamaStatus(); }}
                className={`px-4 py-2 rounded-md text-sm font-medium transition-colors flex items-center gap-2 ${activeTab === 'settings' ? 'bg-white shadow-sm text-gray-900' : 'text-gray-500 hover:text-gray-900'}`}
              >
                <Settings size={16} /> Advanced AI
              </button>
            </div>
          )}
        </div>

        <div className="flex items-center gap-4 flex-shrink-0">
          <button
            onClick={selectFolder}
            disabled={scanning || fileBusy}
            className="ui-button ui-button--primary"
          >
            <FolderOpen size={18} />
            {scanning ? "Scanning..." : "Scan New Folder"}
          </button>
        </div>
      </header>

      {notice && <div role={notice.error ? "alert" : "status"} className={`px-6 py-3 flex flex-wrap gap-3 items-center justify-between border-b ${notice.error ? "bg-red-50 text-red-800" : "bg-blue-50 text-blue-900"}`}>
        <p className="text-sm flex-1">{notice.text}</p><div className="flex gap-3">
          {!!notice.ids?.length && <button disabled={fileBusy} onClick={() => recoverPhotos(notice.ids!)} className="font-semibold underline">Undo / Recover</button>}
          <button onClick={() => { setActiveTab('bin'); setViewer(null); }} className="font-semibold underline">View Bin</button>
          <button aria-label="Dismiss message" onClick={() => setNotice(null)} className="px-2">Dismiss</button>
        </div></div>}
      {aiRunning && aiProgress && aiProgress.total > 0 && (
        <div className="px-6 py-2 bg-blue-50 border-b border-blue-100 flex items-center gap-3" role="status">
          <Cpu size={14} className="text-blue-600 shrink-0" />
          <span className="text-xs text-blue-800 font-medium whitespace-nowrap">AI analyzing {aiProgress.model || "photos"}…</span>
          <div className="flex-1 h-1.5 bg-blue-100 rounded-full overflow-hidden max-w-md">
            <div className="h-1.5 bg-blue-600 rounded-full transition-all duration-300" style={{ width: `${(aiProgress.processed / aiProgress.total) * 100}%` }}></div>
          </div>
          <span className="text-xs text-blue-700 font-medium whitespace-nowrap">{aiProgress.processed}/{aiProgress.total}</span>
          <button onClick={() => invoke("cancel_ai_analysis")} className="ui-button ui-button--secondary text-xs">Cancel</button>
        </div>
      )}
      {scanning && photos.length > 0 && <div role="status" className="px-6 py-3 bg-blue-50 border-b border-blue-100 flex gap-3 items-center">
        <span className="text-sm text-blue-900 flex-1">{scanPaused ? "Paused" : "Analyzing locally"}: {scanProgress?.processed ?? 0} / {scanProgress?.total ?? 0} · {scanProgress?.current_file}</span>
        <button onClick={scanPaused ? resumeScan : pauseScan} className="ui-button ui-button--secondary text-xs">{scanPaused ? "Resume" : "Pause"}</button>
        <button onClick={cancelScan} className="ui-button ui-button--danger text-xs">Cancel</button>
      </div>}
      <main className="flex-1 overflow-hidden">
        {scanning && photos.length === 0 ? (
          <div className="h-full flex flex-col items-center justify-center p-8">
            <div className="w-full max-w-md bg-white p-8 rounded-2xl shadow-sm border border-gray-100 text-center">
              <div className={`inline-block p-4 bg-blue-50 rounded-full mb-4 ${scanPaused ? '' : 'animate-pulse'}`}>
                <FolderOpen size={32} className="text-blue-600" />
              </div>
              <h3 className="text-xl font-medium text-gray-900 mb-2">{scanPaused ? "Scan Paused" : "Analyzing Photos"}</h3>
              <p className="text-gray-500 mb-6 text-sm">
                Creating previews, measuring sharpness and exposure, finding duplicates, and recommending keepers automatically. No Ollama required.
              </p>
              
              {scanProgress && (
                <div className="w-full text-left mb-6">
                  <div className="flex justify-between text-xs text-gray-500 mb-2 font-medium">
                    <span>{scanProgress.processed} / {scanProgress.total}</span>
                    <span>{scanProgress.total > 0 ? Math.round((scanProgress.processed / scanProgress.total) * 100) : 0}%</span>
                  </div>
                  <div className="w-full bg-gray-100 rounded-full h-2.5 mb-3 overflow-hidden">
                    <div 
                      className={`h-2.5 rounded-full transition-all duration-300 ease-out ${scanPaused ? 'bg-gray-400' : 'bg-blue-600'}`}
                      style={{ width: `${scanProgress.total > 0 ? (scanProgress.processed / scanProgress.total) * 100 : 0}%` }}
                    ></div>
                  </div>
                  <p className="text-xs text-gray-400 truncate text-center" title={scanProgress.current_file}>
                    {scanProgress.current_file}
                  </p>
                </div>
              )}

              <div className="flex justify-center gap-3">
                {scanPaused ? (
                  <button onClick={resumeScan} className="ui-button ui-button--primary">
                    Resume
                  </button>
                ) : (
                  <button onClick={pauseScan} className="bg-gray-100 hover:bg-gray-200 text-gray-900 px-4 py-2 rounded-lg text-sm font-medium transition-colors border border-gray-200">
                    Pause
                  </button>
                )}
                <button onClick={cancelScan} className="bg-red-50 hover:bg-red-100 text-red-600 px-4 py-2 rounded-lg text-sm font-medium transition-colors border border-red-100">
                  Cancel
                </button>
              </div>
            </div>
          </div>
        ) : photos.length === 0 && activeTab !== 'bin' && activeTab !== 'settings' ? (
          <div className="h-full flex flex-col items-center justify-center text-center p-8 bg-white">
            <div className="w-24 h-24 bg-blue-50 rounded-3xl flex items-center justify-center mb-8 rotate-3 shadow-inner">
              <ImageIcon size={40} className="text-blue-600 -rotate-3" />
            </div>
            <h2 className="text-4xl font-semibold mb-4 tracking-tight text-gray-900">Your Photos. Your Drive.</h2>
            <p className="text-gray-500 mb-10 text-lg max-w-lg leading-relaxed">
              PhotoMind scans and organizes your library completely locally. No cloud uploads. No subscriptions.
            </p>
            <button
              onClick={selectFolder}
              className="ui-button ui-button--primary"
            >
              Start Local Scan
            </button>
          </div>
        ) : activeTab === 'dashboard' && stats ? (
          <div className="h-full p-6 max-w-7xl mx-auto overflow-y-auto">
            <div className="mb-6"><h2 className="page-title">Dashboard</h2><p className="text-sm text-gray-600 mt-1">Review matches, keep your favourites, and move unwanted photos to Bin.</p></div>
            
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mb-8">
              <div className="bg-white p-6 rounded-2xl border border-gray-200 shadow-sm flex flex-col">
                <h3 className="text-gray-500 font-medium text-sm uppercase tracking-wide mb-2">Total Library Size</h3>
                <div className="flex items-baseline gap-2 mb-2">
                  <span className="text-4xl font-bold text-gray-900">{(stats.total_bytes / 1024 / 1024 / 1024).toFixed(2)}</span>
                  <span className="text-gray-500 font-medium">GB</span>
                </div>
                <p className="text-sm text-gray-500">{stats.total_photos.toLocaleString()} photos indexed</p>
              </div>

              <div className="bg-blue-50 p-6 rounded-2xl border border-blue-100 shadow-sm flex flex-col">
                <h3 className="text-blue-600 font-medium text-sm uppercase tracking-wide mb-2">Potential Cleanup</h3>
                <div className="flex items-baseline gap-2 mb-2">
                  <span className="text-4xl font-bold text-blue-900">
                    {(cleanupBytes / 1024 / 1024).toFixed(1)}
                  </span>
                  <span className="text-blue-700 font-medium">MB</span>
                </div>
                <p className="text-sm text-blue-700 font-medium">Includes visual matches to review. Bin keeps files until permanently deleted.</p>
              </div>
            </div>

            <div className="bg-white rounded-2xl border border-gray-200 shadow-sm overflow-hidden">
              <div className="p-6 border-b border-gray-100">
                <h3 className="text-lg font-bold text-gray-900">Cleanup Opportunities</h3>
              </div>
              <div className="divide-y divide-gray-100">
                
                <div className="cleanup-row">
                  <div>
                    <h4 className="font-semibold text-gray-900 mb-1">Exact Duplicates</h4>
                    <p className="text-sm text-gray-500">Binary-identical files that can be safely removed.</p>
                  </div>
                  <div className="flex flex-wrap items-center gap-3">
                    <div className="text-right">
                      <p className="font-bold text-gray-900">{exactCleanup.ids.length}</p>
                      <p className="text-xs text-gray-500">{(exactCleanup.bytes / 1024 / 1024).toFixed(1)} MB</p>
                    </div>
                    <button 
                      onClick={() => runGroupCleanup("exact")}
                      disabled={fileBusy || exactCleanup.ids.length === 0}
                      className="ui-button ui-button--danger"
                    >
                      <Trash2 size={16} /> Move exact copies to Bin
                    </button>
                  </div>
                </div>

                <div className="cleanup-row">
                  <div><h4 className="font-semibold text-gray-900 mb-1">Visual Matches</h4><p className="text-sm text-gray-600">{duplicates.filter(group => group.kind === "similar").length} groups of similar photos. Review differences before cleanup.</p></div>
                  <div className="flex flex-wrap items-center gap-3">
                    <div className="text-right"><p className="font-bold text-gray-900">{visualCleanup.ids.length} candidates</p><p className="text-xs text-gray-600">{(visualCleanup.bytes / 1048576).toFixed(1)} MB</p></div>
                    <button className="ui-button ui-button--secondary" disabled={!duplicates.some(group => group.kind === "similar")} onClick={() => setActiveTab("duplicates")}>Review matches</button>
                    <button className="ui-button ui-button--danger" disabled={fileBusy || !visualCleanup.ids.length} onClick={() => runGroupCleanup("similar")}>Keep suggested · Bin the rest</button>
                  </div>
                </div>

                <div className="cleanup-row">
                  <div>
                    <h4 className="font-semibold text-gray-900 mb-1">Suggested Removals</h4>
                    <p className="text-sm text-gray-500">Redundant or technically inferior frames with alternatives. Review before removing.</p>
                  </div>
                  <div className="flex flex-wrap items-center gap-3">
                    <div className="text-right">
                      <p className="font-bold text-gray-900">{stats.recommended_removals}</p>
                      <p className="text-xs text-gray-500">{(stats.recommended_removals_bytes / 1024 / 1024).toFixed(1)} MB</p>
                    </div>
                    <button 
                      onClick={() => {
                        const idsToTrash: string[] = [];
                        moments.forEach(momentGroup => {
                          momentGroup.recommendations.forEach(rec => {
                            if (rec.decision === 'REMOVE' && !momentGroup.photos.find(p => p.id === rec.photo_id)?.is_kept) {
                              idsToTrash.push(rec.photo_id);
                            }
                          });
                        });
                        trashPhotos(idsToTrash);
                      }}
                      disabled={fileBusy || stats.recommended_removals === 0}
                      className="ui-button ui-button--danger"
                    >
                      <Trash2 size={16} /> Move recommendations to Bin
                    </button>
                  </div>
                </div>

              </div>
            </div>
          </div>
        ) : activeTab === 'library' ? (
          <div className="h-full p-6 flex flex-col">
            <div className="mb-4 flex items-end justify-between">
              <h2 className="page-title">All Photos</h2>
              <p className="text-sm text-gray-500 font-medium">{stats?.total_photos ?? photos.length} indexed</p>
            </div>
            <VirtuosoGrid
              style={{ flex: 1 }}
              totalCount={photos.length}
              endReached={loadMorePhotos}
              computeItemKey={(index) => photos[index].id}
              listClassName="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 xl:grid-cols-8 gap-4"
              itemClassName="aspect-square"
              itemContent={(index) => {
                const photo = photos[index];
                
                return (
                  <button aria-label={`View ${photo.filename} large`} onClick={() => openViewer(photos, photo)} className="w-full h-full bg-gray-100 rounded-xl overflow-hidden shadow-sm hover:shadow-md transition-all relative group border border-gray-200">
                    <PhotoThumb
                      photoId={photo.id} version={photo.sha256}
                      filename={photo.filename}
                      className="w-full h-full object-cover transition-transform duration-500 group-hover:scale-105"
                    />
                    <span className={`absolute top-2 right-2 text-[10px] font-bold px-2 py-1 rounded ${recommendations.get(photo.id)?.decision === 'KEEP' ? 'bg-green-700 text-white' : recommendations.get(photo.id)?.decision === 'REMOVE' ? 'bg-red-700 text-white' : 'bg-amber-100 text-amber-900'}`}>
                      {recommendations.get(photo.id)?.decision === 'REMOVE' ? 'REJECT' : recommendations.get(photo.id)?.decision ?? (photo.analysis_status === 'error' ? 'REVIEW' : 'Analyzing…')}
                    </span>
                    <div className="absolute inset-0 bg-gradient-to-t from-black/60 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity flex items-end p-3">
                      <p className="text-white text-xs truncate w-full font-medium shadow-sm" title={photo.filename}>
                        {photo.filename}
                      </p>
                    </div>
                  </button>
                );
              }}
            />
          </div>
        ) : activeTab === 'duplicates' ? (
          <div className="h-full p-6 flex flex-col overflow-y-auto">
            <div className="mb-6 flex items-end justify-between">
              <div>
                <h2 className="text-lg font-semibold text-gray-900">Duplicates & Similar Photos</h2>
                <p className="text-sm text-gray-500 mt-1">Matches cover all scanned folders, regardless of filename numbers or capture dates. Exact copies have identical file contents; visual matches need review.</p>
              </div>
              <p className="text-sm text-gray-500 font-medium">{duplicates.length} groups found</p>
            </div>
            
            {duplicates.length === 0 ? (
              <div className="flex-1 flex flex-col items-center justify-center border-2 border-dashed border-gray-200 rounded-2xl">
                <p className="text-gray-700 font-medium">No duplicate or visually similar groups found.</p>
                <p className="mt-2 text-sm text-gray-500">No matching groups were detected in the scanned folders. Scan any other folders you want to include.</p>
              </div>
            ) : (
              <div className="space-y-8 pb-10">
                {duplicates.map((group, groupIdx) => {
                  const keeper = group.photos.find(p => p.is_kept) ?? group.photos[0];
                  return <section key={group.sha256} className="bg-white p-5 rounded-2xl shadow-sm border border-gray-200">
                    <div className="flex flex-wrap justify-between items-center gap-3 mb-4">
                      <div><h3 className="font-semibold text-gray-900">{group.kind === "exact" ? "Exact copies" : "Visual matches"} · Group {groupIdx + 1}</h3><p className="text-xs text-gray-600 mt-1">Compare to change the keeper, or keep it and move the other photos to Bin together.</p></div>
                      <div className="flex gap-2"><button onClick={() => openViewer(group.photos, group.photos.find(p => p.id !== keeper.id) ?? keeper, false, keeper.id)} className="ui-button ui-button--secondary">Compare large</button>
                        <button disabled={fileBusy || !group.photos.some(p => p.id !== keeper.id && !p.is_kept)} onClick={() => fileAction("move_to_trash", group.photos.filter(p => p.id !== keeper.id && !p.is_kept).map(p => p.id), "moved to Bin; the keeper is protected", { photo: keeper, group: group.photos.filter(p => !p.is_kept || p.id === keeper.id) })} className="ui-button ui-button--danger">Keep {keeper.is_kept ? "chosen" : "suggested"} · Bin the rest</button></div>
                    </div>
                    <div className="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-5 gap-4">
                      {group.photos.map(photo => <article key={photo.id} className="flex flex-col gap-2">
                        <button onClick={() => openViewer(group.photos, photo, false, keeper.id)} aria-label={`View ${photo.filename} large`} className={`aspect-square bg-gray-100 rounded-lg overflow-hidden border-2 ${photo.id === keeper.id || photo.is_kept ? 'border-green-600' : 'border-gray-200'}`}>
                          <PhotoThumb photoId={photo.id} version={photo.sha256} filename={photo.filename} className="w-full h-full object-cover" />
                        </button>
                        <p className="text-sm font-medium truncate" title={photo.absolute_path}>{photo.filename}</p><p className="text-xs text-gray-600 truncate" title={photo.absolute_path}>{photo.absolute_path}</p>
                        <p className="text-xs text-gray-600">{(photo.file_size / 1048576).toFixed(2)} MB · {photo.is_kept ? 'Kept / protected' : photo.id === keeper.id ? 'Suggested keeper' : group.kind === 'exact' ? 'Exact copy' : 'Review match'}</p>
                        <div className="flex flex-wrap gap-2">{(photo.id === keeper.id || photo.is_kept) && <span className="rounded-lg bg-green-50 text-green-800 px-3 py-2 text-xs font-semibold">{photo.is_kept ? "Kept" : "Suggested keep"}</span>}
                          <button disabled={fileBusy || photo.is_kept} title={photo.is_kept ? 'Choose another keeper first' : 'Recoverable from Bin'} onClick={() => trashPhotos([photo.id])} className="ui-button ui-button--danger text-xs">Move to Bin</button></div>
                      </article>)}
                    </div>
                  </section>;
                })}
              </div>
            )}
          </div>
        ) : activeTab === 'moments' ? (
          <div className="h-full p-6 flex flex-col overflow-y-auto">
            <div className="mb-6 flex items-end justify-between">
              <div>
                <h2 className="text-lg font-semibold text-gray-900">Photographic Moments</h2>
                <p className="text-sm text-gray-500 mt-1">Bursts and near-duplicates taken at the same time.</p>
              </div>
              <p className="text-sm text-gray-500 font-medium">{moments.length} moments found</p>
            </div>

            <div role="status" className="mb-6 bg-white border border-gray-200 rounded-2xl p-4 shadow-sm">
              <h3 className="text-sm font-semibold text-gray-900 flex items-center gap-2"><Cpu size={16} className="text-blue-600" /> Automatic local analysis</h3>
              <p className="text-xs text-gray-600 mt-1">Recommendations use measured sharpness, clipping and duplicate similarity. Inspect faces and expressions when comparing. Advanced AI is optional.</p>
            </div>

            {moments.length === 0 ? (
              <div className="flex-1 flex flex-col items-center justify-center border-2 border-dashed border-gray-200 rounded-2xl">
                <p className="text-gray-500 font-medium">No moments found.</p>
              </div>
            ) : (
              <div className="space-y-8 pb-10">
                {moments.map((group, groupIdx) => (
                  <div key={group.moment.id} className="bg-white p-5 rounded-2xl shadow-sm border border-gray-200">
                    <div className="flex justify-between items-center mb-4">
                      <div className="flex items-center gap-3">
                        <h3 className="font-semibold text-gray-900 text-sm bg-gray-100 px-3 py-1.5 rounded-lg">
                          Moment #{groupIdx + 1}
                        </h3>
                        <span className="text-xs text-gray-500 font-mono tracking-wider">
                          {group.moment.start_time ? new Date(group.moment.start_time).toLocaleString() : "Unknown Time"}
                        </span>
                      </div>
                      
                      <button onClick={() => {
                        const keeper = group.photos.find(p => p.is_kept) ?? group.photos.find(p => group.recommendations.some(r => r.photo_id === p.id && r.decision === "KEEP")) ?? group.photos[0];
                        if (keeper) openViewer(group.photos, group.photos.find(p => p.id !== keeper.id) ?? keeper, false, keeper.id);
                      }} className="ui-button ui-button--primary">Compare large</button>
                    </div>
                    
                    <div className="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-5 gap-4">
                      {group.photos.map((photo) => {
                        const rec = group.recommendations.find(r => r.photo_id === photo.id);
                        const ai = aiAnalyses.get(photo.id);
                        const face = faceSummaries.get(photo.id);
                        return (
                        <div key={photo.id} className="flex flex-col gap-2">
                          {rec && <p className="text-xs text-gray-600 order-last">{formatReasoning(rec.reasoning)}</p>}
                          <div className={`aspect-square bg-gray-100 rounded-lg overflow-hidden border-2 group relative transition-colors ${
                            rec?.decision === 'KEEP' ? 'border-green-500 shadow-[0_0_15px_rgba(34,197,94,0.2)]' : 
                            rec?.decision === 'REMOVE' ? 'border-red-500 opacity-70' : 
                            rec?.decision === 'REVIEW' ? 'border-yellow-400' : 
                            'border-gray-200'
                          }`}>
                            <PhotoThumb
                              photoId={photo.id} version={photo.sha256}
                              filename={photo.filename}
                              className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                            />
                            
                            {rec && (
                              <div className="absolute top-2 right-2 flex flex-col gap-1 items-end z-10">
                                {rec.best_of_group && rec.best_of_group > 1 && rec.decision === "KEEP" && (
                                  <span className="text-[10px] font-bold px-2 py-0.5 rounded shadow-sm bg-indigo-600 text-white">⭐ BEST OF {rec.best_of_group}</span>
                                )}
                                <span className={`text-[10px] font-bold px-2 py-0.5 rounded shadow-sm ${
                                  rec.decision === 'KEEP' ? 'bg-green-500 text-white' :
                                  rec.decision === 'REMOVE' ? 'bg-red-500 text-white' :
                                  'bg-yellow-400 text-yellow-900'
                                }`}>
                                  {rec.decision === "REMOVE" ? "REJECT" : rec.decision}
                                </span>
                              </div>
                            )}
                            {face && face.face_count > 0 && (
                              <div className="absolute top-2 left-2 flex flex-col gap-1 z-10">
                                {face.closed_eye_warning && (
                                  <span className="text-[10px] font-semibold px-1.5 py-0.5 rounded bg-amber-100 text-amber-800">👁 Possible closed eyes</span>
                                )}
                                {face.uncertain_eyes && !face.closed_eye_warning && (
                                  <span className="text-[10px] font-semibold px-1.5 py-0.5 rounded bg-gray-100 text-gray-700">Eye state uncertain</span>
                                )}
                              </div>
                            )}

                            {rec && (
                              <div className="absolute bottom-2 right-2 flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity z-20">
                                {rec.decision !== 'KEEP' && (
                                  <button onClick={(e) => { e.stopPropagation(); handleUserOverride(photo.id, rec.decision, 'KEEP'); }} className="bg-green-500 hover:bg-green-600 text-white p-1.5 rounded-full shadow-sm" title="Override: Keep">
                                    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
                                  </button>
                                )}
                                {rec.decision !== 'REMOVE' && (
                                  <button onClick={(e) => { e.stopPropagation(); handleUserOverride(photo.id, rec.decision, 'REMOVE'); }} className="bg-red-500 hover:bg-red-600 text-white p-1.5 rounded-full shadow-sm" title="Override: Trash">
                                    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
                                  </button>
                                )}
                              </div>
                            )}

                            <div className="absolute inset-0 bg-black/60 opacity-0 group-hover:opacity-100 transition-opacity flex flex-col items-center justify-center p-1.5 gap-1.5">
                              {rec ? (
                                <p className="text-white text-[10px] leading-snug text-center line-clamp-4 max-h-full overflow-hidden">
                                  {formatReasoning(rec.reasoning)}
                                </p>
                              ) : (
                                <p className="text-white text-[10px] leading-snug text-center line-clamp-4 max-h-full overflow-hidden">Technical analysis pending.</p>
                              )}
                              <button
                                onClick={() => openAiDetails(photo)}
                                disabled={isAnalyzing || !selectedModel}
                                className="shrink-0 bg-blue-600 hover:bg-blue-700 disabled:opacity-40 text-white text-[10px] font-semibold px-2 py-1 rounded shadow-sm"
                              >
                                {isAnalyzing && testAnalysisPhoto?.id === photo.id ? "Analyzing…" : aiAnalyses.has(photo.id) ? "AI details" : "Analyze with AI"}
                              </button>
                            </div>
                          </div>
<div className="text-xs">
            <p className="font-medium text-gray-900 truncate" title={photo.filename}>{photo.filename}</p>
            {face && face.face_count > 0 && (
              <div className="flex flex-wrap gap-1 mt-1.5">
                <span className="bg-blue-50 text-blue-700 rounded px-1.5 py-0.5 text-[10px] font-semibold">{face.face_count} face{face.face_count > 1 ? "s" : ""}</span>
                {!face.closed_eye_warning && !face.uncertain_eyes && (
                  <span className="bg-green-50 text-green-700 rounded px-1.5 py-0.5 text-[10px] font-semibold">Eyes likely open</span>
                )}
                {face.average_smile != null && (
                  <span className="bg-purple-50 text-purple-700 rounded px-1.5 py-0.5 text-[10px] font-semibold">Expression analyzed</span>
                )}
              </div>
            )}
            {ai && (
              <div className="flex flex-wrap gap-1 mt-1.5">
                {ai.blur?.blurred ? (
                  <span className="bg-red-50 text-red-700 rounded px-1.5 py-0.5 text-[10px] font-semibold" title={ai.blur.cause ?? "Blurred photo"}>Blur</span>
                ) : (
                  <span className="bg-green-50 text-green-700 rounded px-1.5 py-0.5 text-[10px] font-semibold">Sharp</span>
                )}
                {(ai.people?.count ?? 0) > 0 && (
                  <span className="bg-blue-50 text-blue-700 rounded px-1.5 py-0.5 text-[10px] font-semibold">{ai.people.count} face{ai.people.count > 1 ? "s" : ""}</span>
                )}
                {(ai.people?.count ?? 0) > 0 && !ai.people.eyes_open && (
                  <span className="bg-amber-50 text-amber-700 rounded px-1.5 py-0.5 text-[10px] font-semibold">Eyes closed</span>
                )}
              </div>
            )}
            {photo.sharpness !== null && (
                              <div className="mt-2 space-y-1">
                                <div className="flex justify-between items-center text-[10px]">
                                  <span className="text-gray-500">Sharpness</span>
                                  <span className="font-medium text-gray-700">{Math.round(photo.sharpness * 100)}%</span>
                                </div>
                                <div className="w-full bg-gray-100 rounded-full h-1">
                                  <div className="bg-blue-500 h-1 rounded-full" style={{ width: `${Math.round(photo.sharpness * 100)}%` }}></div>
                                </div>
                              </div>
                            )}
                          </div>
                        </div>
                      )})}
                    </div>
                  </div>
                ))}
              </div>
            )}

            {testAnalysis && (
              <div className="fixed bottom-0 left-0 right-0 bg-white border-t border-gray-200 shadow-[0_-10px_40px_rgba(0,0,0,0.1)] p-6 z-50 max-h-[55vh] overflow-y-auto">
                <div className="flex flex-wrap justify-between items-center gap-3 mb-4">
                  <h3 className="font-bold text-lg text-gray-900 flex items-center gap-2 min-w-0">
                    <Cpu className="text-blue-600 shrink-0" />
                    <span className="truncate">AI Details — {testAnalysisPhoto?.filename ?? "Photo"}</span>
                  </h3>
                  <button onClick={() => { setTestAnalysis(null); setTestAnalysisPhoto(null); }} className="text-gray-400 hover:text-gray-900 font-bold px-3 py-1 bg-gray-100 rounded shrink-0">Close</button>
                </div>

                {testAnalysisPhoto && (
                  <div className="flex flex-wrap gap-2 mb-4">
                    <button disabled={fileBusy || testAnalysisPhoto.is_kept} onClick={() => { keepPhoto(testAnalysisPhoto, [testAnalysisPhoto]); setTestAnalysisPhoto({ ...testAnalysisPhoto, is_kept: true }); }} className="rounded-lg bg-green-50 hover:bg-green-100 text-green-800 disabled:opacity-40 px-3 py-1.5 text-xs font-semibold">
                      {testAnalysisPhoto.is_kept ? "Marked Keep" : "Mark as Keep"}
                    </button>
                    <button disabled={fileBusy || testAnalysisPhoto.is_kept} title={testAnalysisPhoto.is_kept ? "Choose another keeper before moving to Bin" : "Recoverable from Bin"} onClick={() => trashPhotos([testAnalysisPhoto.id])} className="rounded-lg bg-red-50 hover:bg-red-100 text-red-700 disabled:opacity-40 px-3 py-1.5 text-xs font-semibold">
                      Move to Bin
                    </button>
                  </div>
                )}

                <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
                  <div>
                    <h4 className="font-semibold text-gray-700 text-sm mb-2 uppercase tracking-wide">Overview</h4>
                    <p className="text-sm"><span className="font-medium">Scene:</span> {testAnalysis.scene_type}</p>
                    <p className="text-sm"><span className="font-medium">Aesthetic:</span> {testAnalysis.aesthetic_score}/10</p>
                    {testAnalysis.blur && (
                      <p className="text-sm mt-1">
                        <span className="font-medium">Blur:</span>{" "}
                        {testAnalysis.blur.blurred ? (
                          <span className="text-red-600 font-semibold">Blurred ({(testAnalysis.blur.level * 100).toFixed(0)}%){testAnalysis.blur.cause ? ` · ${testAnalysis.blur.cause}` : ""}</span>
                        ) : (
                          <span className="text-green-600 font-semibold">Sharp</span>
                        )}
                      </p>
                    )}
                    <p className="text-xs mt-2 text-gray-600 italic line-clamp-3" title={testAnalysis.summary}>"{testAnalysis.summary}"</p>
                  </div>

                  <div>
                    <h4 className="font-semibold text-gray-700 text-sm mb-2 uppercase tracking-wide">People & Faces</h4>
                    <p className="text-sm"><span className="font-medium">Faces:</span> {testAnalysis.people.count} · <span className="font-medium">Eyes open:</span> {testAnalysis.people.eyes_open ? "Yes" : "No"} · <span className="font-medium">Looking at camera:</span> {testAnalysis.people.looking_at_camera ? "Yes" : "No"}</p>
                    <p className="text-sm"><span className="font-medium">Expression:</span> {(testAnalysis.people.expression_quality * 100).toFixed(0)}% · <span className="font-medium">Face quality:</span> {(testAnalysis.people.face_quality * 100).toFixed(0)}%</p>
                    {testAnalysis.faces && testAnalysis.faces.length > 0 && (
                      <ul className="mt-2 space-y-1">
                        {testAnalysis.faces.map((face, i) => (
                          <li key={i} className="flex items-center gap-2 text-xs">
                            <span className={`px-1.5 py-0.5 rounded font-semibold ${face.eye_state === "CLOSED" ? "bg-red-100 text-red-700" : face.eye_state === "OPEN" ? "bg-green-100 text-green-800" : "bg-amber-100 text-amber-800"}`}>{face.eye_state}</span>
                            <span>Face {i + 1}: {face.emotion ? face.emotion : "no expression"} · Q{(face.quality * 100).toFixed(0)}%</span>
                            <span className="text-gray-500">{face.looking_at_camera ? "camera" : "not camera"}</span>
                          </li>
                        ))}
                      </ul>
                    )}
                    <p className="text-sm mt-2"><span className="font-medium">Subject clear:</span> {testAnalysis.subject.clear ? "Yes" : "No"} · <span className="font-medium">Quality:</span> {testAnalysis.subject.quality}/10</p>
                  </div>

                  <div>
                    <h4 className="font-semibold text-gray-700 text-sm mb-2 uppercase tracking-wide">Composition & Problems</h4>
                    <p className="text-sm"><span className="font-medium">Score:</span> {testAnalysis.composition.score}/10{testAnalysis.composition.issues?.length ? ` · ${testAnalysis.composition.issues.join(" · ")}` : ""}</p>
                    {testAnalysis.problems.length > 0 ? (
                      <ul className="list-disc pl-4 mt-1 text-xs text-red-600 space-y-0.5 line-clamp-4">
                        {testAnalysis.problems.map((p, i) => <li key={i} className="truncate" title={p}>{p}</li>)}
                      </ul>
                    ) : (
                      <p className="text-sm text-green-600 mt-1">No major problems detected.</p>
                    )}
                  </div>
                </div>

                {testAnalysis.suggestions && testAnalysis.suggestions.length > 0 && (
                  <div className="mt-5 border-t border-gray-100 pt-4">
                    <h4 className="font-semibold text-gray-700 text-sm mb-2 uppercase tracking-wide">Suggestions</h4>
                    <ul className="grid grid-cols-1 md:grid-cols-2 gap-x-6 gap-y-1">
                      {testAnalysis.suggestions.map((s, i) => (
                        <li key={i} className="text-xs text-gray-700 flex items-start gap-1.5">
                          <span className="text-blue-600 font-bold mt-px">▸</span>
                          <span>{s}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}
              </div>
            )}
          </div>
        ) : activeTab === 'bin' ? (
          <div className="h-full p-6 overflow-y-auto">
            <div className="flex flex-wrap justify-between items-center gap-4 mb-6"><div><h2 id="bin-title" tabIndex={-1} className="page-title">Bin</h2><p className="text-sm text-gray-600 mt-1">Review removed photos large, recover them to their original folders, or delete permanently.</p><p className="text-xs text-gray-600 mt-1">On macOS, these files are in system Trash. Emptying Trash in Finder also removes them permanently.</p></div><button disabled={fileBusy || binPhotos.length === 0} onClick={() => recoverPhotos(binPhotos.map(item => item.photo.id))} className="ui-button ui-button--primary">Recover all</button></div>
            {binPhotos.length === 0 ? <div className="border-2 border-dashed rounded-2xl p-16 text-center"><Trash2 className="mx-auto text-gray-500 mb-4" size={32} /><h3 className="font-semibold">Bin is empty</h3><p className="text-sm text-gray-600 mt-2">Photos moved to Bin will stay here for review and recovery.</p></div> :
              <div className="grid grid-cols-2 md:grid-cols-4 lg:grid-cols-5 gap-5">{binPhotos.map(item => <article key={item.photo.id} className="bg-white border rounded-xl p-3 flex flex-col gap-3">
                <button onClick={() => openViewer(binPhotos.map(b => b.photo), item.photo, true)} aria-label={`View ${item.photo.filename} in Bin large`} className="aspect-square rounded-lg overflow-hidden bg-gray-100"><PhotoThumb photoId={item.photo.id} version={item.photo.sha256} filename={item.photo.filename} className="w-full h-full object-cover" /></button>
                <div><p className="text-sm font-semibold truncate" title={item.photo.filename}>{item.photo.filename}</p><p className="text-xs text-gray-600 truncate mt-1" title={item.original_path}>From: {item.original_path}</p><p className="text-xs text-gray-600 mt-1">{(item.photo.file_size / 1048576).toFixed(2)} MB</p>{!item.available && <p className="text-xs text-red-700 mt-2">File unavailable. It may have been recovered or Trash was emptied in Finder.</p>}</div>
                <button disabled={fileBusy} onClick={() => recoverPhotos([item.photo.id])} className="rounded-lg bg-blue-50 hover:bg-blue-100 text-blue-800 disabled:opacity-40 px-3 py-2 text-sm font-semibold">Recover</button>
                <button disabled={fileBusy || !item.available} onClick={() => purgePhotos([item.photo.id])} className="ui-button ui-button--danger text-xs">Delete permanently…</button>
              </article>)}</div>}
          </div>
        ) : activeTab === 'settings' ? (
          <div className="h-full p-8 flex flex-col max-w-3xl mx-auto overflow-y-auto">
            <h2 className="page-title mb-6">Advanced AI & Privacy</h2>
            <p className="text-sm text-gray-600 mb-6">Optional photo descriptions and experimental face/expression estimates. Automatic scanning and recommendations work without this service.</p>
            
            <div className="bg-white rounded-2xl border border-gray-200 p-6 shadow-sm mb-6">
              <div className="flex items-center justify-between mb-4">
                <h3 className="text-lg font-medium flex items-center gap-2">
                  <Cpu size={20} className={ollamaError ? "text-red-500" : "text-green-500"} />
                  Local AI Status
                </h3>
                <button 
                  onClick={checkOllamaStatus}
                  className="text-sm bg-gray-100 hover:bg-gray-200 px-3 py-1.5 rounded-lg font-medium transition-colors"
                >
                  Refresh
                </button>
              </div>
              
              {ollamaError ? (
                <div className="bg-red-50 text-red-700 p-4 rounded-xl text-sm mb-4">
                  <p className="font-semibold mb-1">Ollama is not running</p>
                  <p>{ollamaError}</p>
                  <p className="mt-2 text-red-600/80">Make sure you have Ollama installed and running locally on port 11434. PhotoMind will NEVER use cloud AI APIs to protect your privacy.</p>
                </div>
              ) : (
                <div className="bg-green-50 text-green-800 p-4 rounded-xl text-sm mb-6 flex items-center gap-2 font-medium">
                  <div className="w-2 h-2 rounded-full bg-green-500 animate-pulse"></div>
                  Connected to local Ollama runtime
                </div>
              )}

              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-2">Selected Vision Model</label>
                  <select 
                    value={selectedModel || ""}
                    onChange={(e) => setSelectedModel(e.target.value)}
                    disabled={!ollamaModels || ollamaModels.length === 0}
                    className="w-full bg-gray-50 border border-gray-200 text-gray-900 rounded-lg focus:ring-blue-500 focus:border-blue-500 block p-2.5"
                  >
                    {!ollamaModels || ollamaModels.length === 0 ? (
                      <option>No models installed...</option>
                    ) : (
                      ollamaModels.map(m => (
                        <option key={m.name} value={m.name}>
                          {m.name} {m.is_vision ? "(Vision Supported)" : "(No Vision Detected)"}
                        </option>
                      ))
                    )}
                  </select>
                  {ollamaModels && ollamaModels.length > 0 && selectedModel && !ollamaModels.find(m => m.name === selectedModel)?.is_vision && (
                    <p className="text-amber-600 text-xs mt-2 font-medium">
                      Warning: The selected model may not support image analysis. 
                      Try installing a model like `llama3.2-vision` or `moondream`.
                    </p>
                  )}
                </div>
              </div>
            </div>
            
            <div className="bg-white rounded-2xl border border-gray-200 p-6 shadow-sm">
              <h3 className="text-lg font-medium mb-4 text-gray-900">Privacy Constraints</h3>
              
              <label className="flex items-start gap-3 cursor-pointer">
                <div className="flex items-center h-5 mt-0.5">
                  <input 
                    type="checkbox" 
                    checked={localProcessingOnly}
                    onChange={(e) => setLocalProcessingOnly(e.target.checked)}
                    className="w-4 h-4 text-blue-600 bg-gray-100 border-gray-300 rounded focus:ring-blue-500" 
                  />
                </div>
                <div className="text-sm">
                  <span className="font-medium text-gray-900 block mb-1">Strict Local Processing Only</span>
                  <p className="text-gray-500">
                    When enabled, the application will forcefully reject any attempt to route requests to remote APIs or remote instances of Ollama.
                    All processing and inference occurs locally on your machine.
                  </p>
                </div>
              </label>
            </div>
          </div>
        ) : null}
      </main>
      {viewer && <PhotoViewer viewer={viewer} busy={fileBusy} message={notice?.text} onViewBin={() => { setViewer(null); setActiveTab("bin"); }} onClose={() => setViewer(null)} onSelect={id => setViewer(previous => previous ? { ...previous, selectedId: id } : null)} onKeep={keepPhoto} onBin={trashPhotos} onRecover={recoverPhotos} onPurge={purgePhotos} />}
    </div>
  );
}

export default App;
