export interface Photo {
  analysis_status?: string;
  id: string;
  is_kept: boolean;
  absolute_path: string;
  filename: string;
  extension: string;
  file_size: number;
  width: number | null;
  height: number | null;
  capture_time: string | null;
  sha256: string | null;
  thumbnail_path: string | null;
  sharpness: number | null;
  exposure: number | null;
  contrast: number | null;
  highlight_clipping: number | null;
  shadow_clipping: number | null;
}
export interface BinPhoto {
  photo: Photo;
  original_path: string;
  deleted_at: string;
  available: boolean;
}
export interface FileActionReport {
  photo_ids: string[];
  bytes: number;
  errors: string[];
}
export interface ViewerState {
  photos: Photo[];
  selectedId: string;
  keeperId?: string;
  inBin: boolean;
}

export interface DuplicateGroup {
  sha256: string;
  count: number;
  kind: "exact" | "similar";
  photos: Photo[];
}
