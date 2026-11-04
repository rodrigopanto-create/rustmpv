package com.visualrustmpv.app;

import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

final class SiteCookies {
    private SiteCookies() {}

    static String root(String domain) {
        String normalized = (domain.startsWith(".") ? domain.substring(1) : domain)
                .toLowerCase(Locale.ROOT);
        for (String root : new String[] {
                "youtube.com", "youtube-nocookie.com",
                "x.com", "twitter.com", "instagram.com", "instagr.am",
                "facebook.com", "fb.com", "fb.watch"
        }) {
            if (normalized.equals(root) || normalized.endsWith("." + root)) {
                if (root.equals("youtube-nocookie.com")) return "youtube.com";
                if (root.equals("x.com")) return "twitter.com";
                if (root.equals("instagr.am")) return "instagram.com";
                if (root.equals("fb.com") || root.equals("fb.watch")) return "facebook.com";
                return root;
            }
        }
        return null;
    }

    private static String lineRoot(String line) {
        String[] fields = line.split("\\t", -1);
        if (fields.length != 7) return null;
        String domain = fields[0].startsWith("#HttpOnly_")
                ? fields[0].substring("#HttpOnly_".length()) : fields[0];
        return root(domain);
    }

    static byte[] merge(byte[] stored, byte[] imported) {
        String incoming = new String(imported, StandardCharsets.UTF_8);
        Set<String> replaced = new HashSet<>();
        for (String line : incoming.split("\\r?\\n")) {
            String root = lineRoot(line);
            if (root != null) replaced.add(root);
        }
        StringBuilder result = new StringBuilder("# Netscape HTTP Cookie File\n");
        for (String line : new String(stored, StandardCharsets.UTF_8).split("\\r?\\n")) {
            String root = lineRoot(line);
            if (root != null && !replaced.contains(root)) result.append(line).append('\n');
        }
        for (String line : incoming.split("\\r?\\n")) {
            if (lineRoot(line) != null) result.append(line).append('\n');
        }
        return result.toString().getBytes(StandardCharsets.UTF_8);
    }
}
