"use client";
import Image from "next/image";
import styles from "./page.module.css";
import { useState } from "react";

export default function Home() {
  const [message, setMessage] = useState("");

  const handlePress = async () => {
    setMessage("");
    try {
      const res = await fetch(
        process.env.NEXT_PUBLIC_API_URL
          ? process.env.NEXT_PUBLIC_API_URL + "/log-frontend-message"
          : "http://localhost:8000/log-frontend-message",
        {
          method: "POST",
        }
      );
      if (res.ok) {
        setMessage("Successfully logged to backend!");
      } else {
        setMessage("Failed to log to backend.");
      }
    } catch (err) {
      setMessage("Error connecting to backend.");
    }
  };

  return (
    <div className={styles.page}>
      <main className={styles.main}>
        <h1 className="text-3xl font-bold mb-4">Hello World</h1>
        <button
          className="bg-blue-500 hover:bg-blue-700 text-white font-bold py-2 px-4 rounded mb-4"
          onClick={handlePress}
        >
          press here
        </button>
        {message && <p className="mt-2 text-green-600">{message}</p>}
      </main>
    </div>
  );
}
