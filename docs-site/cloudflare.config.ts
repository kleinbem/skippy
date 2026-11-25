import { defineConfig, defineWorker } from "cf/config";

export default defineConfig(({ mode }) => {
  const beta = mode === "beta";

  return {
    accountId: "d9b901b29c44e0c48163aace3aa90cfd",
    worker: defineWorker({
      name: beta ? "skippy-docs-beta" : "skippy-docs",
      compatibilityDate: "2026-11-18",
      assets: {
        htmlHandling: "force-trailing-slash",
        notFoundHandling: "404-page",
      },
      domains: [beta ? "beta.skippy.sh" : "skippy.sh"],
    }),
  };
});
