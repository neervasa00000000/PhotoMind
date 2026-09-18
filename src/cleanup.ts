import type { DuplicateGroup, Photo } from "./types";

export function planCleanup(groups: DuplicateGroup[], kind?: DuplicateGroup["kind"]) {
  const keepers = groups.map(group => ({ photo: group.photos.find(p => p.is_kept) ?? group.photos[0], group: group.photos, kind: group.kind })).filter(entry => entry.photo);
  const protectedIds = new Set(keepers.map(entry => entry.photo.id));
  groups.forEach(group => group.photos.forEach(p => { if (p.is_kept) protectedIds.add(p.id); }));
  const removals = new Map<string, Photo>();
  groups.filter(group => !kind || group.kind === kind).forEach(group => group.photos.forEach(p => {
    if (!protectedIds.has(p.id)) removals.set(p.id, p);
  }));
  return {
    ids: [...removals.keys()],
    bytes: [...removals.values()].reduce((sum, photo) => sum + photo.file_size, 0),
    keepers: keepers.filter(entry => !kind || entry.kind === kind).map(entry => ({ ...entry, group: entry.group.filter(p => p.id === entry.photo.id || !protectedIds.has(p.id)) })),
  };
}
