import React, { useState } from "react";

export function SearchPage() {
  const [query, setQuery] = useState("");

  return (
    <div className="search-page">
      <h1>Library Search</h1>
      <div className="search-bar">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search libraries..."
        />
      </div>
      <p className="hint">
        Type to search for JavaScript libraries and frameworks.
      </p>
    </div>
  );
}
