// SPDX-License-Identifier: Apache-2.0

using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;

namespace BragaNet.Tests;

/// <summary>
/// A hermetic fixture: a temporary tree with a sample document, a 32-byte key file, and a loopback
/// OpenAI-compatible embeddings stub, mirroring <c>crates/braga-ffi/tests/golden/harness.rs</c>.
/// </summary>
internal sealed class Harness : IDisposable
{
    /// <summary>The sample document the semantic round-trip indexes.</summary>
    internal const string Sample = "# Notes\n\nThe quick brown fox jumps over the lazy dog.\n";

    /// <summary>The query text that must find <see cref="Sample"/>.</summary>
    internal const string Query = "quick brown fox";

    private readonly string _root;
    private readonly TcpListener _listener;
    private readonly CancellationTokenSource _cancellation = new();
    private readonly Task _server;

    internal Harness()
    {
        _root = Path.Combine(Path.GetTempPath(), $"braga-dotnet-{Guid.NewGuid():N}");
        string docs = Path.Combine(_root, "docs");
        Directory.CreateDirectory(docs);
        File.WriteAllText(Path.Combine(docs, "notes.md"), Sample);

        Key = Path.Combine(_root, "key");
        File.WriteAllBytes(Key, Enumerable.Repeat((byte)0x5A, 32).ToArray());

        _listener = new TcpListener(IPAddress.Loopback, 0);
        _listener.Start();
        int port = ((IPEndPoint)_listener.LocalEndpoint).Port;
        _server = Task.Run(Serve);

        string url = $"http://127.0.0.1:{port}/v1/embeddings";
        Config = Path.Combine(_root, "braga.toml");
        File.WriteAllText(
            Config,
            "[scan]\n"
                + $"dir = {Toml(docs)}\n\n"
                + "[embedding]\n"
                + "provider = \"openai-compatible\"\n"
                + $"url = {Toml(url)}\n"
                + "model = \"stub-embed\"\n\n"
                + "[storage]\n"
                + $"path = {Toml(Path.Combine(_root, "index.db"))}\n");
    }

    /// <summary>The fixture root, which also holds the index database.</summary>
    internal string Root => _root;

    /// <summary>The stub-backed configuration file.</summary>
    internal string Config { get; }

    /// <summary>The 32-byte key file.</summary>
    internal string Key { get; }

    /// <summary>Writes another file in the fixture root and returns its path.</summary>
    internal string Write(string name, byte[] contents)
    {
        string path = Path.Combine(_root, name);
        File.WriteAllBytes(path, contents);
        return path;
    }

    /// <inheritdoc />
    public void Dispose()
    {
        _cancellation.Cancel();
        _listener.Stop();
        try
        {
            _server.Wait(TimeSpan.FromSeconds(5));
        }
        catch (AggregateException)
        {
            // The listener was stopped mid-accept; nothing to report in a fixture teardown.
        }

        _cancellation.Dispose();
        try
        {
            Directory.Delete(_root, recursive: true);
        }
        catch (IOException)
        {
            // A leaked store handle must not fail teardown; the OS temp directory is disposable.
        }
    }

    private void Serve()
    {
        while (!_cancellation.IsCancellationRequested)
        {
            TcpClient client;
            try
            {
                client = _listener.AcceptTcpClient();
            }
            catch (SocketException)
            {
                return;
            }
            catch (ObjectDisposedException)
            {
                return;
            }
            catch (InvalidOperationException)
            {
                return;
            }

            _ = Task.Run(() => Handle(client));
        }
    }

    private static void Handle(TcpClient client)
    {
        using (client)
        {
            try
            {
                Respond(client.GetStream());
            }
            catch (IOException)
            {
                // A truncated request just closes the connection; the caller sees EMBED.
            }
            catch (SocketException)
            {
                // The peer went away mid-response; the caller sees EMBED.
            }
            catch (JsonException)
            {
                // A malformed request body is not ours to repair; the caller sees EMBED.
            }
            catch (ObjectDisposedException)
            {
                // Teardown closed the listener under an in-flight connection.
            }
        }
    }

    /// <summary>Answers one request with one <c>[1,0,0,0]</c> vector per input, the Rust golden vector.</summary>
    private static void Respond(NetworkStream stream)
    {
        byte[] header = ReadHeader(stream);
        byte[] body = ReadBody(stream, ContentLength(header));

        using JsonDocument request = JsonDocument.Parse(body);
        int inputs = 0;
        if (request.RootElement.ValueKind == JsonValueKind.Object
            && request.RootElement.TryGetProperty("input", out JsonElement input)
            && input.ValueKind == JsonValueKind.Array)
        {
            inputs = input.GetArrayLength();
        }

        var json = new StringBuilder("{\"data\":[");
        for (int index = 0; index < inputs; index++)
        {
            if (index > 0)
            {
                json.Append(',');
            }

            json.Append("{\"index\":").Append(index).Append(",\"embedding\":[1,0,0,0]}");
        }

        json.Append("]}");
        byte[] payload = Encoding.UTF8.GetBytes(json.ToString());
        string head = "HTTP/1.1 200 OK\r\n"
            + "Content-Type: application/json\r\n"
            + $"Content-Length: {payload.Length}\r\n"
            + "Connection: close\r\n\r\n";
        stream.Write(Encoding.ASCII.GetBytes(head));
        stream.Write(payload);
        stream.Flush();
    }

    private static byte[] ReadHeader(NetworkStream stream)
    {
        var header = new List<byte>(512);
        while (true)
        {
            int next = stream.ReadByte();
            if (next < 0)
            {
                throw new IOException("connection closed before the request headers");
            }

            header.Add((byte)next);
            if (header.Count >= 4
                && header[^4] == (byte)'\r'
                && header[^3] == (byte)'\n'
                && header[^2] == (byte)'\r'
                && header[^1] == (byte)'\n')
            {
                return header.ToArray();
            }

            if (header.Count > 64 * 1024)
            {
                throw new IOException("request headers are too large");
            }
        }
    }

    private static int ContentLength(byte[] header)
    {
        string text = Encoding.ASCII.GetString(header);
        foreach (string line in text.Split("\r\n", StringSplitOptions.RemoveEmptyEntries))
        {
            int colon = line.IndexOf(':', StringComparison.Ordinal);
            if (colon > 0
                && line[..colon].Trim().Equals("Content-Length", StringComparison.OrdinalIgnoreCase)
                && int.TryParse(line[(colon + 1)..].Trim(), out int length))
            {
                return length;
            }
        }

        return 0;
    }

    private static byte[] ReadBody(NetworkStream stream, int length)
    {
        byte[] body = new byte[length];
        int read = 0;
        while (read < length)
        {
            int count = stream.Read(body, read, length - read);
            if (count == 0)
            {
                throw new IOException("connection closed before the request body");
            }

            read += count;
        }

        return body;
    }

    /// <summary>Renders a TOML basic string for a path or URL.</summary>
    private static string Toml(string value) =>
        "\"" + value.Replace("\\", "\\\\", StringComparison.Ordinal).Replace("\"", "\\\"", StringComparison.Ordinal) + "\"";
}
