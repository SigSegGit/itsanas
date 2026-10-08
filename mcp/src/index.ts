import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { exec } from "child_process";
import { promisify } from "util";

const execAsync = promisify(exec);

const server = new Server(
  {
    name: "itsanas-mcp",
    version: "0.1.0",
  },
  {
    capabilities: {
      tools: {},
    },
  }
);

// Helper pour exécuter les commandes CLI itsanas
async function runItsanasCommand(subcommand: string, instance?: string) {
  const instanceFlag = instance ? `--instance ${instance}` : "";
  const command = `itsanas ${instanceFlag} ${subcommand}`.trim();

  try {
    const { stdout, stderr } = await execAsync(command);
    return stdout || stderr || "Commande exécutée avec succès sans sortie.";
  } catch (error: any) {
    return `Erreur lors de l'exécution ('${command}') : ${error.message}\n${error.stderr || ""}`;
  }
}

// Déclaration des outils
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      {
        name: "itsanas_status",
        description: "Affiche le statut, l'identité et l'hébergement du nœud ITSaNAS.",
        inputSchema: {
          type: "object",
          properties: {
            instance: {
              type: "string",
              description: "Nom de l'instance optionnelle (ex: 'test' ou 'nicolas').",
            },
          },
        },
      },
      {
        name: "itsanas_instances",
        description: "Liste les nœuds/instances ITSaNAS présents sur cette machine.",
        inputSchema: {
          type: "object",
          properties: {},
        },
      },
      {
        name: "itsanas_sync_now",
        description: "Force une ronde de synchronisation immédiate sur l'instance spécifiée.",
        inputSchema: {
          type: "object",
          properties: {
            instance: { type: "string", description: "Nom de l'instance" },
          },
        },
      },
      {
        name: "itsanas_doctor",
        description: "Vérifie l'intégrité de tous les blocs stockés par le nœud local.",
        inputSchema: {
          type: "object",
          properties: {
            instance: { type: "string", description: "Nom de l'instance" },
          },
        },
      },
      {
        name: "itsanas_exec",
        description: "Exécute n'importe quelle commande CLI ITSaNAS autorisée.",
        inputSchema: {
          type: "object",
          properties: {
            subcommand: {
              type: "string",
              description: "La sous-commande complète (ex: 'whoami', 'space', 'pause --for 1h').",
            },
            instance: { type: "string", description: "Nom de l'instance optionnelle." },
          },
          required: ["subcommand"],
        },
      },
    ],
  };
});

// Traitement des requêtes
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;
  const instance = args?.instance as string | undefined;

  switch (name) {
    case "itsanas_status": {
      const result = await runItsanasCommand("status", instance);
      return { content: [{ type: "text", text: result }] };
    }
    case "itsanas_instances": {
      const result = await runItsanasCommand("instances");
      return { content: [{ type: "text", text: result }] };
    }
    case "itsanas_sync_now": {
      const result = await runItsanasCommand("sync-now", instance);
      return { content: [{ type: "text", text: result }] };
    }
    case "itsanas_doctor": {
      const result = await runItsanasCommand("doctor", instance);
      return { content: [{ type: "text", text: result }] };
    }
    case "itsanas_exec": {
      const subcommand = args?.subcommand as string;
      const result = await runItsanasCommand(subcommand, instance);
      return { content: [{ type: "text", text: result }] };
    }
    default:
      throw new Error(`Outil inconnu : ${name}`);
  }
});

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch(console.error);