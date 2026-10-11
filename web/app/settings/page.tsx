import { Suspense } from "react";
import SettingsForm from "@/components/SettingsForm";
import Topbar from "@/components/Topbar";

export default function SettingsPage() {
  return (
    <Suspense fallback={<Topbar title="Settings" settingsLink={false} />}>
      <SettingsForm />
    </Suspense>
  );
}
