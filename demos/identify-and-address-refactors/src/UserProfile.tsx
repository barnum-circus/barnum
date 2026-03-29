import React, { useState, useEffect } from "react";

type User = {
  id: number;
  firstName: string;
  lastName: string;
  email: string;
  age: number;
  role: "admin" | "user" | "moderator";
};

// Fetches user data and displays profile info.
// Riddled with unnecessary useEffects and useStates for derived values.
export function UserProfile({ userId }: { userId: number }) {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Derived state stored in useState + synced with useEffect
  const [fullName, setFullName] = useState("");
  const [isAdmin, setIsAdmin] = useState(false);
  const [displayAge, setDisplayAge] = useState("");
  const [emailDomain, setEmailDomain] = useState("");
  const [initials, setInitials] = useState("");

  useEffect(() => {
    setLoading(true);
    setError(null);
    fetch(`/api/users/${userId}`)
      .then((res) => {
        if (!res.ok) throw new Error("Failed to fetch");
        return res.json();
      })
      .then((data) => {
        setUser(data);
        setLoading(false);
      })
      .catch((err) => {
        setError(err.message);
        setLoading(false);
      });
  }, [userId]);

  // All of these are just derived from `user` — no useEffect needed
  useEffect(() => {
    if (user) {
      setFullName(`${user.firstName} ${user.lastName}`);
    }
  }, [user]);

  useEffect(() => {
    if (user) {
      setIsAdmin(user.role === "admin");
    }
  }, [user]);

  useEffect(() => {
    if (user) {
      setDisplayAge(`${user.age} years old`);
    }
  }, [user]);

  useEffect(() => {
    if (user) {
      setEmailDomain(user.email.split("@")[1] || "");
    }
  }, [user]);

  useEffect(() => {
    if (user) {
      setInitials(
        user.firstName.charAt(0).toUpperCase() +
          user.lastName.charAt(0).toUpperCase()
      );
    }
  }, [user]);

  if (loading) return <div>Loading...</div>;
  if (error) return <div style={{ color: "red" }}>Error: {error}</div>;
  if (!user) return null;

  return (
    <div
      style={{
        border: "1px solid #ccc",
        padding: "16px",
        borderRadius: "8px",
        maxWidth: "400px",
      }}
    >
      <div
        style={{
          width: "48px",
          height: "48px",
          borderRadius: "50%",
          backgroundColor: "#3b82f6",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: "white",
          fontWeight: "bold",
          fontSize: "18px",
          marginBottom: "12px",
        }}
      >
        {initials}
      </div>
      <h2 style={{ margin: "0 0 4px 0" }}>{fullName}</h2>
      <p style={{ color: "#666", margin: "0 0 8px 0" }}>{user.email}</p>
      <p style={{ margin: "0 0 4px 0" }}>Age: {displayAge}</p>
      <p style={{ margin: "0 0 4px 0" }}>Domain: {emailDomain}</p>
      {isAdmin && (
        <span
          style={{
            backgroundColor: "#ef4444",
            color: "white",
            padding: "2px 8px",
            borderRadius: "4px",
            fontSize: "12px",
          }}
        >
          Admin
        </span>
      )}
    </div>
  );
}
