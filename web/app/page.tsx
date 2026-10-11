import { Suspense } from "react";
import Main from "@/components/Main";

export default function Page() {
  return (
    <Suspense fallback={null}>
      <Main />
    </Suspense>
  );
}
