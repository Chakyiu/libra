/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "#0b0f14",
        panel: "#121821",
        edge: "#1e2733",
        ink: "#e6edf3",
        mute: "#8b98a5",
        accent: "#4ea1ff",
      },
    },
  },
  plugins: [],
};
