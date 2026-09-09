import { BrowserRouter, Route, Routes } from "react-router-dom";

import { About } from "@/pages/About";
import { Landing } from "@/pages/Landing";
import { Pool } from "@/pages/Pool";
import { Terminal } from "@/pages/Terminal";

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Landing />} />
        <Route path="/trade" element={<Terminal />} />
        <Route path="/pool" element={<Pool />} />
        <Route path="/about" element={<About />} />
        {/* Anything else is the landing page rather than a 404 — this is a small site. */}
        <Route path="*" element={<Landing />} />
      </Routes>
    </BrowserRouter>
  );
}
