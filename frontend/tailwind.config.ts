import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        lxcup: {
          ink: "#172033",
          muted: "#647089",
          paper: "#f4f6fa",
          panel: "#ffffff",
          line: "#dce2eb",
          primary: "#2563eb",
          success: "#16845b",
          warning: "#a66700",
          danger: "#c53c50",
          nav: "#18243a",
        },
      },
    },
  },
  plugins: [],
} satisfies Config;
