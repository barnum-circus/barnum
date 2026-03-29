import React, { useState, useEffect, useRef } from "react";

type Metric = {
  label: string;
  value: number;
  change: number;
};

type Notification = {
  id: number;
  message: string;
  read: boolean;
  timestamp: string;
};

// Dashboard component with excessive state management anti-patterns.
// - useEffect for things that should be event handlers
// - useRef to "track previous value" instead of useMemo
// - Storing window dimensions in state (should use CSS or a hook)
// - Multiple useEffects that could be one
export function Dashboard() {
  const [metrics, setMetrics] = useState<Metric[]>([]);
  const [notifications, setNotifications] = useState<Notification[]>([]);
  const [selectedTab, setSelectedTab] = useState<"overview" | "notifications">(
    "overview"
  );
  const [refreshInterval, setRefreshInterval] = useState(30);

  // Anti-pattern: storing window dimensions in state
  const [windowWidth, setWindowWidth] = useState(window.innerWidth);
  const [windowHeight, setWindowHeight] = useState(window.innerHeight);

  // Anti-pattern: tracking "previous" metrics with useRef + useEffect
  const prevMetricsRef = useRef<Metric[]>([]);
  const [metricsChanged, setMetricsChanged] = useState(false);

  // Derived state in useState
  const [unreadCount, setUnreadCount] = useState(0);
  const [totalValue, setTotalValue] = useState(0);
  const [positiveMetrics, setPositiveMetrics] = useState<Metric[]>([]);
  const [negativeMetrics, setNegativeMetrics] = useState<Metric[]>([]);
  const [isCompact, setIsCompact] = useState(false);

  // Window resize listener
  useEffect(() => {
    const handler = () => {
      setWindowWidth(window.innerWidth);
      setWindowHeight(window.innerHeight);
    };
    window.addEventListener("resize", handler);
    return () => window.removeEventListener("resize", handler);
  }, []);

  // Compact mode derived from window width — should be CSS media query
  useEffect(() => {
    setIsCompact(windowWidth < 768);
  }, [windowWidth]);

  // Fetch metrics
  useEffect(() => {
    const fetchMetrics = () => {
      fetch("/api/metrics")
        .then((res) => res.json())
        .then((data) => {
          setMetrics(data);
        });
    };
    fetchMetrics();
    const interval = setInterval(fetchMetrics, refreshInterval * 1000);
    return () => clearInterval(interval);
  }, [refreshInterval]);

  // Fetch notifications
  useEffect(() => {
    fetch("/api/notifications")
      .then((res) => res.json())
      .then(setNotifications);
  }, []);

  // Track previous metrics and detect changes
  useEffect(() => {
    if (
      JSON.stringify(prevMetricsRef.current) !== JSON.stringify(metrics)
    ) {
      setMetricsChanged(true);
      setTimeout(() => setMetricsChanged(false), 2000);
    }
    prevMetricsRef.current = metrics;
  }, [metrics]);

  // Sync unread count
  useEffect(() => {
    setUnreadCount(notifications.filter((n) => !n.read).length);
  }, [notifications]);

  // Sync total value
  useEffect(() => {
    setTotalValue(metrics.reduce((sum, m) => sum + m.value, 0));
  }, [metrics]);

  // Sync positive metrics
  useEffect(() => {
    setPositiveMetrics(metrics.filter((m) => m.change > 0));
  }, [metrics]);

  // Sync negative metrics
  useEffect(() => {
    setNegativeMetrics(metrics.filter((m) => m.change < 0));
  }, [metrics]);

  // Anti-pattern: useEffect as event handler for marking notifications read
  const [notificationToMark, setNotificationToMark] = useState<number | null>(
    null
  );
  useEffect(() => {
    if (notificationToMark !== null) {
      setNotifications((prev) =>
        prev.map((n) =>
          n.id === notificationToMark ? { ...n, read: true } : n
        )
      );
      setNotificationToMark(null);
    }
  }, [notificationToMark]);

  const markAsRead = (id: number) => {
    setNotificationToMark(id);
  };

  return (
    <div style={{ padding: isCompact ? "8px" : "24px" }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: "16px",
        }}
      >
        <h1 style={{ margin: 0 }}>Dashboard</h1>
        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          <span style={{ fontSize: "12px", color: "#999" }}>
            {windowWidth}×{windowHeight}
          </span>
          <select
            value={refreshInterval}
            onChange={(e) => setRefreshInterval(Number(e.target.value))}
          >
            <option value={10}>10s</option>
            <option value={30}>30s</option>
            <option value={60}>60s</option>
          </select>
        </div>
      </div>

      <div style={{ display: "flex", gap: "16px", marginBottom: "16px" }}>
        <button
          onClick={() => setSelectedTab("overview")}
          style={{
            fontWeight: selectedTab === "overview" ? "bold" : "normal",
          }}
        >
          Overview
        </button>
        <button
          onClick={() => setSelectedTab("notifications")}
          style={{
            fontWeight:
              selectedTab === "notifications" ? "bold" : "normal",
          }}
        >
          Notifications {unreadCount > 0 && `(${unreadCount})`}
        </button>
      </div>

      {selectedTab === "overview" && (
        <div>
          {metricsChanged && (
            <div
              style={{
                backgroundColor: "#dbeafe",
                padding: "8px",
                borderRadius: "4px",
                marginBottom: "8px",
              }}
            >
              Metrics updated!
            </div>
          )}
          <div style={{ marginBottom: "8px" }}>
            Total: {totalValue} | Up: {positiveMetrics.length} | Down:{" "}
            {negativeMetrics.length}
          </div>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: isCompact ? "1fr" : "1fr 1fr 1fr",
              gap: "12px",
            }}
          >
            {metrics.map((metric, i) => (
              <div
                key={i}
                style={{
                  border: "1px solid #e5e7eb",
                  borderRadius: "8px",
                  padding: "16px",
                }}
              >
                <div style={{ color: "#6b7280", fontSize: "14px" }}>
                  {metric.label}
                </div>
                <div
                  style={{
                    fontSize: "24px",
                    fontWeight: "bold",
                  }}
                >
                  {metric.value}
                </div>
                <div
                  style={{
                    color: metric.change > 0 ? "#22c55e" : "#ef4444",
                    fontSize: "14px",
                  }}
                >
                  {metric.change > 0 ? "+" : ""}
                  {metric.change}%
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {selectedTab === "notifications" && (
        <div>
          {notifications.map((notif) => (
            <div
              key={notif.id}
              style={{
                padding: "12px",
                borderBottom: "1px solid #eee",
                backgroundColor: notif.read ? "transparent" : "#f0f9ff",
                cursor: "pointer",
              }}
              onClick={() => markAsRead(notif.id)}
            >
              <div>{notif.message}</div>
              <div style={{ color: "#999", fontSize: "12px" }}>
                {new Date(notif.timestamp).toLocaleString()}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
