import MediaSearch from "@/components/MediaSearch";

export default function ImagesPage() {
  return (
    <MediaSearch
      kind="images"
      title="Images"
      hint="Image search through the configured providers — SearxNG, Brave, Bing and Tavily, in your fallback order."
    />
  );
}
