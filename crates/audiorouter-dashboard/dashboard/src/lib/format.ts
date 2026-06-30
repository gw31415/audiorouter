/**
 * Format helper — channel list as comma-separated string.
 * Mirrors `tui.rs::channel_label`.
 */
export function channelLabel(channels: number[]): string {
  return channels.join(",");
}
