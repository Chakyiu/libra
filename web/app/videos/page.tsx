import MediaSearch from "@/components/MediaSearch";

export default function VideosPage() {
  return (
    <MediaSearch
      kind="videos"
      title="Videos"
      hint="Video search works best with SearxNG, which gathers YouTube, Vimeo and Dailymotion results."
    />
  );
}
