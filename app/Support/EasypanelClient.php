<?php
// app/Support/EasypanelClient.php
namespace App\Support;

use Illuminate\Support\Facades\Http;

class EasypanelClient
{
    public function __construct(
        private string $url,
        private string $token,
    ) {}

    /**
     * Panggil endpoint tRPC EasyPanel dan kembalikan payload `.json`.
     */
    public function call(string $group, string $op, mixed $input = null): mixed
    {
        $base = rtrim($this->url, '/');

        $response = Http::withToken($this->token)
            ->acceptJson()
            ->post("{$base}/api/rpc/{$group}/{$op}", ['json' => $input]);

        if ($response->failed()) {
            throw EasypanelException::fromResponse($response);
        }

        return $response->json('json');
    }
}
